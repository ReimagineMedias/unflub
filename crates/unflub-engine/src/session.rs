//! The handshake, for both ends. The app calls [`serve`] with the pid it spawned; the engine
//! calls [`join`] with the pid of the app that spawned it.

use std::io::{self, Read};
use std::time::{Duration, Instant};

use crate::frame;
use crate::pipe::{Cancellation, Listener, Pipe};
use crate::protocol::{FromEngine, ToEngine, PROTOCOL};

/// How long either side waits for the other during the handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many clients that aren't the engine the app drops in one handshake before it gives up.
pub const MAX_IMPOSTORS: u32 = 3;

/// A finished handshake on the app's side.
pub struct Served {
    pub pipe: Pipe,
    /// The engine's version.
    pub version: String,
    /// Clients that took the pipe first and were dropped because they weren't the engine.
    pub impostors: u32,
}

/// The app's side: accept the engine `engine_pid`, check who it is and what it speaks, and
/// welcome it. Returns the pipe and the engine's version.
///
/// A client with another process id is dropped and the pipe listens again, so another process
/// that grabs the one instance first can't keep the engine out (#123); the engine waits for
/// the busy pipe ([`Pipe::connect_within`]). After [`MAX_IMPOSTORS`] such clients, or one
/// followed by no engine before the deadline, the start fails with `PermissionDenied`.
pub fn serve(
    listener: Listener,
    engine_pid: u32,
    app_exe: &str,
    timeout: Duration,
) -> io::Result<(Pipe, String)> {
    serve_inner(listener, engine_pid, app_exe, timeout, None).map(|s| (s.pipe, s.version))
}

/// Like [serve], but interrupt a pending connection or Hello when the app stops startup.
pub fn serve_cancellable(
    listener: Listener,
    engine_pid: u32,
    app_exe: &str,
    timeout: Duration,
    cancellation: Cancellation<'_>,
) -> io::Result<Served> {
    serve_inner(listener, engine_pid, app_exe, timeout, Some(cancellation))
}

fn serve_inner(
    listener: Listener,
    engine_pid: u32,
    app_exe: &str,
    timeout: Duration,
    cancellation: Option<Cancellation<'_>>,
) -> io::Result<Served> {
    let deadline = Instant::now() + timeout;
    let mut listener = listener;
    let mut impostors = 0;
    // Once a client was dropped, every failure says how many (`impostors_in`), so the app can
    // log a squatted pipe however the start then ended.
    let with_count = |e: io::Error, impostors: u32| match impostors {
        0 => e,
        count => Impostors::wrap(e, count, engine_pid),
    };
    let pipe = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let pipe =
            match listener.accept_with_cancel(left.max(Duration::from_millis(1)), cancellation) {
                Ok(pipe) => pipe,
                Err(e) if impostors > 0 && e.kind() == io::ErrorKind::TimedOut => {
                    return Err(Impostors::refused(impostors, engine_pid))
                }
                Err(e) => return Err(with_count(e, impostors)),
            };
        if pipe.client_pid().map_err(|e| with_count(e, impostors))? == engine_pid {
            break pipe;
        }
        impostors += 1;
        if impostors >= MAX_IMPOSTORS {
            return Err(Impostors::refused(impostors, engine_pid));
        }
        listener = pipe.disconnect().map_err(|e| with_count(e, impostors))?;
    };
    welcome(pipe, app_exe, deadline, cancellation)
        .map(|(pipe, version)| Served {
            pipe,
            version,
            impostors,
        })
        .map_err(|e| with_count(e, impostors))
}

/// The rest of the app's handshake with the right client: its Hello, our Welcome.
fn welcome(
    pipe: Pipe,
    app_exe: &str,
    deadline: Instant,
    cancellation: Option<Cancellation<'_>>,
) -> io::Result<(Pipe, String)> {
    let version = match read_until(&pipe, deadline, cancellation)? {
        FromEngine::Hello { protocol, version } if protocol == PROTOCOL => version,
        FromEngine::Hello { protocol, version } => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("engine {version} speaks protocol {protocol}, the app {PROTOCOL}"),
            ))
        }
        _ => return Err(unexpected()),
    };
    let welcome = ToEngine::Welcome {
        protocol: PROTOCOL,
        app_exe: app_exe.to_string(),
    };
    match cancellation {
        Some(cancellation) => frame::write(&mut pipe.cancellable_writer(cancellation), &welcome)?,
        None => frame::write(&mut &pipe, &welcome)?,
    }
    Ok((pipe, version))
}

/// Inside the `io::Error` of a handshake that dropped clients which weren't the engine.
#[derive(Debug)]
struct Impostors {
    count: u32,
    engine_pid: u32,
    /// What ended the handshake afterwards; `None` when the impostors did.
    cause: Option<io::Error>,
}

impl Impostors {
    /// The impostors ended it: too many, or no engine came after them.
    fn refused(count: u32, engine_pid: u32) -> io::Error {
        let inner = Self {
            count,
            engine_pid,
            cause: None,
        };
        io::Error::new(io::ErrorKind::PermissionDenied, inner)
    }

    /// Something else ended it after `count` impostors; the kind stays the cause's.
    fn wrap(cause: io::Error, count: u32, engine_pid: u32) -> io::Error {
        let kind = cause.kind();
        let inner = Self {
            count,
            engine_pid,
            cause: Some(cause),
        };
        io::Error::new(kind, inner)
    }
}

impl std::fmt::Display for Impostors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dropped = format!(
            "{} pipe clients were not the engine {}",
            self.count, self.engine_pid
        );
        match &self.cause {
            Some(cause) => write!(f, "{cause} ({dropped})"),
            None => f.write_str(&dropped),
        }
    }
}

impl std::error::Error for Impostors {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

/// How many clients that weren't the engine a failed [`serve`] or [`serve_cancellable`]
/// dropped first, if any.
pub fn impostors_in(error: &io::Error) -> Option<u32> {
    error
        .get_ref()?
        .downcast_ref::<Impostors>()
        .map(|i| i.count)
}

/// The engine's side: connect, check the server is the app (`app_pid`), say hello and wait
/// for the welcome. Returns the pipe and the app's executable name.
pub fn join(name: &str, app_pid: u32, timeout: Duration) -> io::Result<(Pipe, String)> {
    let deadline = Instant::now() + timeout;
    let pipe = Pipe::connect_within(name, timeout)?;
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
    match read_until(&pipe, deadline, None)? {
        ToEngine::Welcome { protocol, app_exe } if protocol == PROTOCOL => Ok((pipe, app_exe)),
        ToEngine::Welcome { protocol, .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the app speaks protocol {protocol}, the engine {PROTOCOL}"),
        )),
        _ => Err(unexpected()),
    }
}

fn unexpected() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unexpected message during the handshake",
    )
}

/// One message, read before `deadline`.
fn read_until<T: serde::de::DeserializeOwned>(
    pipe: &Pipe,
    deadline: Instant,
    cancellation: Option<Cancellation<'_>>,
) -> io::Result<T> {
    let body =
        read_frame_until(pipe, deadline, cancellation)?.ok_or(io::ErrorKind::UnexpectedEof)?;
    frame::decode(&body)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid handshake message"))
}

/// Read one whole frame within the time limit, including the length prefix.
pub fn read_frame(pipe: &Pipe, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
    read_frame_until(pipe, Instant::now() + timeout, None)
}

fn read_frame_until(
    pipe: &Pipe,
    deadline: Instant,
    cancellation: Option<Cancellation<'_>>,
) -> io::Result<Option<Vec<u8>>> {
    frame::read(&mut Deadline {
        pipe,
        deadline,
        cancellation,
    })
}

struct Deadline<'a> {
    pipe: &'a Pipe,
    deadline: Instant,
    cancellation: Option<Cancellation<'a>>,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.pipe.read_with_cancel(buf, left, self.cancellation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::unique_name;
    use std::sync::atomic::{AtomicU64, Ordering};
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
        // We are the client, but we claim to have spawned process 1. The impostor is dropped
        // and no engine follows before the deadline.
        let err = serve(listener, 1, "unflub.exe", Duration::from_millis(500))
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(impostors_in(&err), Some(1), "the failure says why");
        assert!(
            engine.join().unwrap().is_err(),
            "no welcome for an impostor"
        );
    }

    /// #123: a process that keeps grabbing the pipe is dropped each time, and the start fails
    /// after a few of them rather than waiting out the deadline.
    #[test]
    fn repeated_impostors_end_the_attempt() {
        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let squatter = thread::spawn({
            let stop = stop.clone();
            move || {
                let mut held = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(pipe) = Pipe::connect_within(&name, Duration::from_millis(200)) {
                        held.push(pipe);
                    }
                }
                held.len()
            }
        });
        let started = Instant::now();
        let err = serve(listener, 1, "unflub.exe", T).err().unwrap();
        stop.store(true, Ordering::Relaxed);
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(impostors_in(&err), Some(MAX_IMPOSTORS));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "didn't wait for the deadline"
        );
        assert_eq!(squatter.join().unwrap() as u32, MAX_IMPOSTORS);
    }

    /// Review L2: a failure after dropping clients keeps its kind and still carries the count.
    #[test]
    fn a_later_failure_still_says_how_many_were_dropped() {
        let err = Impostors::wrap(io::ErrorKind::TimedOut.into(), 2, 9);
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(impostors_in(&err), Some(2));
        assert!(err
            .to_string()
            .contains("2 pipe clients were not the engine 9"));
        assert_eq!(impostors_in(&io::ErrorKind::TimedOut.into()), None);
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
        assert_eq!(impostors_in(&err), None, "no client was dropped");
        drop(fake.join());
    }

    fn with_cancellation<T>(test: impl FnOnce(Cancellation<'_>) -> T) -> T {
        let epoch = AtomicU64::new(0);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                epoch.store(1, Ordering::Relaxed);
            });
            test(Cancellation::new(&epoch, 0))
        })
    }

    #[test]
    fn a_stalled_handshake_is_cancellable_before_and_after_connect() {
        for connect in [false, true] {
            let name = unique_name();
            let listener = Listener::create(&name).unwrap();
            let _silent_peer = connect.then(|| Pipe::connect(&name).unwrap());
            let started = Instant::now();
            let err = with_cancellation(|cancellation| {
                serve_cancellable(listener, std::process::id(), "unflub.exe", T, cancellation)
                    .err()
                    .unwrap()
            });
            assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
            assert!(started.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn initial_settings_write_can_be_cancelled_if_the_peer_does_not_read() {
        use std::io::Write;

        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let _silent_peer = Pipe::connect(&name).unwrap();
        let pipe = listener.accept(T).unwrap();
        let started = Instant::now();
        let err = with_cancellation(|cancellation| {
            pipe.cancellable_writer(cancellation)
                .write_all(&[0; 256 * 1024])
                .unwrap_err()
        });
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
        assert!(started.elapsed() < crate::pipe::WRITE_TIMEOUT);
    }

    #[test]
    fn a_partial_frame_times_out_while_the_peer_stays_connected() {
        use std::io::Write;

        for prefix in [&[10, 0][..], &[10, 0, 0, 0, b'{'][..]] {
            let name = unique_name();
            let listener = Listener::create(&name).unwrap();
            let peer = Pipe::connect(&name).unwrap();
            let pipe = listener.accept(T).unwrap();
            (&peer).write_all(prefix).unwrap();
            let err = read_frame(&pipe, Duration::from_millis(100)).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        }
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
