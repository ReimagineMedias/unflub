//! The installed app's CLI. A CLI invocation never initializes Tauri or starts autocorrect.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use unflub_engine::{
    frame,
    pipe::{Listener, Pipe},
};
use uuid::Uuid;

use crate::{
    controls::{self, Pause},
    current_settings, update_settings_checked, AppState,
};

const ENDPOINT_FILE: &str = "control-v1.json";
const BUNDLE_ID: &str = "ca.rmgn.unflub";
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const USAGE: &str =
    "Usage: unflub.exe --get key | --set key=value | --pause [15m|1h|restart] | --resume";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Get { key: String },
    Set { key: String, value: Value },
    Pause { duration: PauseFor },
    Resume,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PauseFor {
    Minutes15,
    Hour,
    Restart,
}

fn parse(args: &[&str]) -> Result<Request, &'static str> {
    let key = |key: &str| {
        if key.is_empty() {
            Err("A settings key is required.")
        } else {
            Ok(key.to_owned())
        }
    };
    match args {
        ["--get", name] => Ok(Request::Get { key: key(name)? }),
        ["--set", assignment] => {
            let (name, input) = assignment.split_once('=').ok_or("Use --set key=value.")?;
            let value = serde_json::from_str(input).unwrap_or_else(|_| Value::String(input.into()));
            Ok(Request::Set {
                key: key(name)?,
                value,
            })
        }
        ["--pause"] | ["--pause", "restart"] => Ok(Request::Pause {
            duration: PauseFor::Restart,
        }),
        ["--pause", "15m"] => Ok(Request::Pause {
            duration: PauseFor::Minutes15,
        }),
        ["--pause", "1h"] => Ok(Request::Pause {
            duration: PauseFor::Hour,
        }),
        ["--resume"] => Ok(Request::Resume),
        _ => Err(USAGE),
    }
}

pub fn dispatch(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let args: Vec<_> = args.into_iter().collect();
    if args.is_empty()
        || matches!(args.as_slice(), [flag] if flag == crate::BACKGROUND_ARG)
        || matches!(args.as_slice(), [flag, dir] if flag == crate::PERF_IDLE_ARG && !dir.is_empty())
    {
        return None;
    }
    // A release build uses the Windows subsystem so a normal tray launch has no console.
    // A scripted launch can still inherit redirected handles or attach to its parent console.
    // SAFETY: this changes console attachment only in the CLI process. Debug builds already
    // have a console, in which case AttachConsole fails and the existing handles are kept.
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    let args: Option<Vec<_>> = args.iter().map(|arg| arg.to_str()).collect();
    let result = match args.as_deref() {
        Some(["--help" | "-h"]) => {
            println!("{USAGE}");
            return Some(0);
        }
        Some(["--version"]) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return Some(0);
        }
        Some(args) => parse(args).map_err(str::to_owned).and_then(send),
        None => Err("The command contains text unflub couldn't read.".into()),
    };
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
            Some(0)
        }
        Err(message) => {
            eprintln!("unflub: {message}");
            Some(1)
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Endpoint {
    id: Uuid,
    pid: u32,
}

impl Endpoint {
    fn pipe_name(&self) -> String {
        format!(r"\\.\pipe\unflub-control-v1-{}", self.id)
    }

    fn read(path: &Path) -> io::Result<Self> {
        let source = std::fs::File::open(path)?;
        serde_json::from_reader(source.take(1024)).map_err(io::Error::other)
    }
}

type Response = Result<Value, String>;

fn send(request: Request) -> Response {
    let local = dirs::data_local_dir().ok_or("Couldn't find the unflub data folder.")?;
    let path = local.join(BUNDLE_ID).join(ENDPOINT_FILE);
    let unavailable = "Couldn't reach unflub. Start it from the tray or Start menu and try again.";
    let endpoint = Endpoint::read(&path).map_err(|_| unavailable)?;
    let until = Instant::now() + Duration::from_secs(2);
    let name = endpoint.pipe_name();
    let pipe = loop {
        match Pipe::connect(&name) {
            Ok(pipe) => break pipe,
            Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(30)),
            Err(_) => return Err(unavailable.into()),
        }
    };
    if pipe.server_pid().ok() != Some(endpoint.pid) {
        return Err(unavailable.into());
    }
    frame::write(&mut &pipe, &request).map_err(|_| "Couldn't send the command.")?;
    let result = read_frame(&pipe, COMMAND_TIMEOUT + IO_TIMEOUT)
        .map_err(|_| "unflub didn't confirm the command. Use --get to check the settings.")?;
    // Tell the server we read its response before it closes the pipe.
    let _ = (&pipe).write_all(&[1]);
    result
}

fn read_frame<T: serde::de::DeserializeOwned>(pipe: &Pipe, timeout: Duration) -> io::Result<T> {
    let until = Instant::now() + timeout;
    let reader = &mut |buf: &mut [u8]| {
        pipe.read_timeout(buf, until.saturating_duration_since(Instant::now()))
    };
    struct DeadlineRead<'a, F>(&'a mut F);
    impl<F: FnMut(&mut [u8]) -> io::Result<usize>> Read for DeadlineRead<'_, F> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0(buf)
        }
    }
    let bytes = frame::read(&mut DeadlineRead(reader))?.ok_or(io::ErrorKind::UnexpectedEof)?;
    frame::decode(&bytes).map_err(io::Error::other)
}

pub fn start(app: &AppHandle, local_dir: PathBuf) -> io::Result<()> {
    let path = local_dir.join(ENDPOINT_FILE);
    let endpoint = Endpoint {
        id: Uuid::new_v4(),
        pid: std::process::id(),
    };
    let name = endpoint.pipe_name();
    // The existing engine pipe helper restricts clients to this SID and rejects remote clients.
    // Publish the random endpoint in the current user's local data folder only after it exists.
    let listener = Listener::create(&name)?;
    crate::files::replace(&path, &serde_json::to_vec(&endpoint)?)?;
    let app = app.clone();
    std::thread::Builder::new()
        .name("unflub-control".into())
        .spawn(move || {
            serve(&app, &name, listener);
        })?;
    Ok(())
}

fn serve(app: &AppHandle, name: &str, mut listener: Listener) {
    while !app
        .state::<AppState>()
        .stopping
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        match listener.accept(Duration::from_secs(60)) {
            Ok(pipe) => {
                let response = match read_frame(&pipe, IO_TIMEOUT) {
                    Ok(request) => run_request(app, request),
                    Err(_) => Err("Invalid command.".into()),
                };
                if frame::write(&mut &pipe, &response).is_ok() {
                    let _ = pipe.read_timeout(&mut [0], IO_TIMEOUT);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
            Err(e) => eprintln!("unflub control pipe: {e}"),
        }
        match Listener::create(name) {
            Ok(next) => listener = next,
            Err(e) => {
                eprintln!("unflub control pipe: {e}");
                return;
            }
        }
    }
}

/// Process CLI mutations on the same thread as the tray and synchronous settings commands.
fn run_request(app: &AppHandle, request: Request) -> Response {
    let (tx, rx) = mpsc::sync_channel(1);
    let until = Instant::now() + COMMAND_TIMEOUT;
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if Instant::now() < until
            && !handle
                .state::<AppState>()
                .stopping
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            let _ = tx.send(handle_request(&handle, request));
        }
    })
    .map_err(|_| "unflub is shutting down.")?;
    rx.recv_timeout(COMMAND_TIMEOUT)
        .map_err(|_| "unflub didn't confirm the command. Use --get to check the settings.")?
}

fn handle_request(app: &AppHandle, request: Request) -> Response {
    match request {
        Request::Get { key } => {
            let settings =
                serde_json::to_value(current_settings(app)).map_err(|e| e.to_string())?;
            settings
                .get(key)
                .cloned()
                .ok_or_else(|| "Unknown settings key.".into())
        }
        Request::Set { key, value } => {
            let key_to_read = key.clone();
            let patch = [(key, value)].into_iter().collect();
            let saved = update_settings_checked(app, |s| s.patch(patch, false))
                .map_err(|e| e.to_string())?;
            serde_json::to_value(saved)
                .map_err(|e| e.to_string())?
                .get(&key_to_read)
                .cloned()
                .ok_or_else(|| "Couldn't read the saved setting.".into())
        }
        Request::Pause { duration } => {
            if !current_settings(app).enabled {
                return Err("Autocorrect is already switched off.".into());
            }
            match duration {
                PauseFor::Minutes15 => controls::pause_for(app, 15),
                PauseFor::Hour => controls::pause_for(app, 60),
                PauseFor::Restart => controls::set_pause(app, Pause::UntilRestart),
            }
            Ok(Value::Bool(true))
        }
        Request::Resume => {
            if !current_settings(app).enabled {
                return Err(
                    "Autocorrect is switched off. Use --set enabled=true to turn it on.".into(),
                );
            }
            controls::set_pause(app, Pause::Running);
            Ok(Value::Bool(true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_json_and_plain_values() {
        assert_eq!(
            parse(&["--set", "enabled=false"]),
            Ok(Request::Set {
                key: "enabled".into(),
                value: Value::Bool(false),
            })
        );
        assert_eq!(
            parse(&["--set", "language=en-GB"]),
            Ok(Request::Set {
                key: "language".into(),
                value: Value::String("en-GB".into()),
            })
        );
        assert_eq!(
            parse(&["--pause", "1h"]),
            Ok(Request::Pause {
                duration: PauseFor::Hour
            })
        );
        assert!(parse(&["--set", "=bad"]).is_err());
        assert!(parse(&["--pase"]).is_err());
        assert!(parse(&["--get", "enabled", "--pause"]).is_err());
    }

    #[test]
    fn endpoint_uses_the_installed_bundle_directory() {
        let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], BUNDLE_ID);
    }

    #[test]
    fn a_command_and_response_cross_the_local_pipe_without_starting_the_app() {
        let endpoint = Endpoint {
            id: Uuid::new_v4(),
            pid: std::process::id(),
        };
        let name = endpoint.pipe_name();
        let listener = Listener::create(&name).unwrap();
        let server = std::thread::spawn(move || {
            let pipe = listener.accept(IO_TIMEOUT).unwrap();
            assert_eq!(
                read_frame::<Request>(&pipe, IO_TIMEOUT).unwrap(),
                Request::Get {
                    key: "enabled".into()
                }
            );
            frame::write(&mut &pipe, &Ok::<_, String>(Value::Bool(true))).unwrap();
            assert_eq!(pipe.read_timeout(&mut [0], IO_TIMEOUT).unwrap(), 1);
        });
        let pipe = Pipe::connect(&name).unwrap();
        assert_eq!(pipe.server_pid().unwrap(), endpoint.pid);
        frame::write(
            &mut &pipe,
            &Request::Get {
                key: "enabled".into(),
            },
        )
        .unwrap();
        let received: Response = read_frame(&pipe, IO_TIMEOUT).unwrap();
        assert_eq!(received, Ok(Value::Bool(true)));
        (&pipe).write_all(&[1]).unwrap();
        server.join().unwrap();
    }
}
