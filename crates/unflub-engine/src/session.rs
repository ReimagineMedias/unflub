//! The handshake, for both ends. The app calls [`serve`] with the pid it spawned; the engine
//! calls [`join`] with the pid of the app that spawned it.

use std::io::{self, Read};
use std::time::{Duration, Instant};

use crate::frame;
use crate::pipe::{Listener, Pipe};
use crate::protocol::{FromEngine, ToEngine, PROTOCOL};

/// How long either side waits for the other during the handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The app's side: accept the engine `engine_pid`, check who it is and what it speaks, and
/// welcome it. Returns the pipe and the engine's version.
pub fn serve(
    listener: Listener,
    engine_pid: u32,
    app_exe: &str,
    timeout: Duration,
) -> io::Result<(Pipe, String)> {
    let deadline = Instant::now() + timeout;
    let pipe = listener.accept(timeout)?;
    let client = pipe.client_pid()?;
    if client != engine_pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("pipe client is process {client}, not the engine {engine_pid}"),
        ));
    }
    let version = match read_until(&pipe, deadline)? {
        FromEngine::Hello { protocol, version } if protocol == PROTOCOL => version,
        FromEngine::Hello { protocol, version } => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("engine {version} speaks protocol {protocol}, the app {PROTOCOL}"),
            ))
        }
        other => return Err(unexpected(&other)),
    };
    frame::write(
        &mut &pipe,
        &ToEngine::Welcome {
            protocol: PROTOCOL,
            app_exe: app_exe.to_string(),
        },
    )?;
    Ok((pipe, version))
}

/// The engine's side: connect, check the server is the app (`app_pid`), say hello and wait
/// for the welcome. Returns the pipe and the app's executable name.
pub fn join(name: &str, app_pid: u32, timeout: Duration) -> io::Result<(Pipe, String)> {
    let deadline = Instant::now() + timeout;
    let pipe = Pipe::connect(name)?;
    let server = pipe.server_pid()?;
    if server != app_pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("pipe server is process {server}, not the app {app_pid}"),
        ));
    }
    frame::write(
        &mut &pipe,
        &FromEngine::Hello {
            protocol: PROTOCOL,
            version: env!("CARGO_PKG_VERSION").into(),
        },
    )?;
    match read_until(&pipe, deadline)? {
        ToEngine::Welcome { protocol, app_exe } if protocol == PROTOCOL => Ok((pipe, app_exe)),
        ToEngine::Welcome { protocol, .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the app speaks protocol {protocol}, the engine {PROTOCOL}"),
        )),
        other => Err(unexpected(&other)),
    }
}

fn unexpected(message: &impl std::fmt::Debug) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected message during the handshake: {message:?}"),
    )
}

/// One message, read before `deadline`.
fn read_until<T: serde::de::DeserializeOwned>(pipe: &Pipe, deadline: Instant) -> io::Result<T> {
    let mut reader = Deadline { pipe, deadline };
    let body = frame::read(&mut reader)?.ok_or(io::ErrorKind::UnexpectedEof)?;
    frame::decode(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

struct Deadline<'a> {
    pipe: &'a Pipe,
    deadline: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.pipe.read_timeout(buf, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::unique_name;
    use std::thread;

    const T: Duration = Duration::from_secs(5);

    #[test]
    fn both_sides_agree_in_process() {
        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let engine = thread::spawn(move || join(&name, std::process::id(), T).unwrap());
        let (app_pipe, version) = serve(listener, std::process::id(), "unflub.exe", T).unwrap();
        let (engine_pipe, app_exe) = engine.join().unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        assert_eq!(app_exe, "unflub.exe");
        frame::write(&mut &engine_pipe, &FromEngine::Ready { hooked: false }).unwrap();
        let ready: FromEngine =
            frame::decode(&frame::read(&mut &app_pipe).unwrap().unwrap()).unwrap();
        assert_eq!(ready, FromEngine::Ready { hooked: false });
    }

    #[test]
    fn the_app_refuses_a_client_it_did_not_spawn() {
        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let engine = thread::spawn(move || join(&name, std::process::id(), T));
        // We are the client, but we claim to have spawned process 1.
        let err = serve(listener, 1, "unflub.exe", T).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            engine.join().unwrap().is_err(),
            "no welcome for an impostor"
        );
    }

    #[test]
    fn the_engine_refuses_a_server_that_is_not_the_app() {
        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let app = thread::spawn(move || listener.accept(T));
        let err = join(&name, 1, T).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        drop(app.join());
    }

    #[test]
    fn a_protocol_mismatch_is_refused() {
        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let fake = thread::spawn(move || {
            let pipe = Pipe::connect(&name).unwrap();
            frame::write(
                &mut &pipe,
                &FromEngine::Hello {
                    protocol: PROTOCOL + 1,
                    version: "9.9.9".into(),
                },
            )
            .unwrap();
            pipe
        });
        let err = serve(listener, std::process::id(), "unflub.exe", T)
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(fake.join());
    }

    #[test]
    fn a_silent_engine_times_out() {
        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let silent = thread::spawn(move || Pipe::connect(&name).unwrap());
        let started = Instant::now();
        let err = serve(
            listener,
            std::process::id(),
            "unflub.exe",
            Duration::from_millis(200),
        )
        .err()
        .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(silent.join());
    }
}
