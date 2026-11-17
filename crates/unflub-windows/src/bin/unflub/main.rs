//! `unflub`: headless runner for the Windows adapter.
//!
//! ```text
//! unflub --demo [--exclude <exe>]...   three fixed rules (teh, adn, recieve); try it now
//! unflub [--exclude <exe>]...          the real engine (until the core engine lands, it panics)
//! ```
//!
//! Prints each correction and undo with the hook callback time. Ctrl+C unhooks, prints the
//! hook latency histogram and exits.

#[cfg_attr(not(windows), allow(dead_code))]
mod demo;

const USAGE: &str = "usage: unflub [--demo] [--exclude <exe>]...

  --demo           use three built-in rules (teh->the, adn->and, recieve->receive)
                   instead of the engine
  --exclude <exe>  never correct in this app (e.g. code.exe); repeatable
  -h, --help       show this help";

#[derive(Debug, Default, PartialEq)]
struct Args {
    demo: bool,
    exclude: Vec<String>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => out.demo = true,
            "--exclude" => {
                let exe = args.next().ok_or("--exclude needs an exe name")?;
                out.exclude.push(exe.to_lowercase());
            }
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Some(out))
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return;
        }
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    #[cfg(windows)]
    win::run(args);
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("unflub currently runs on Windows only");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    use unflub_core::{Config, Engine, SpellBackend};
    use unflub_windows::{Corrector, Hook, HookEvent, HookOptions, WindowsSpellBackend};
    use windows::Win32::Foundation::{BOOL, TRUE};
    use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_CLOSE_EVENT};

    use super::{demo::DemoCorrector, Args};

    static STOP: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
        STOP.store(true, Ordering::SeqCst);
        if kind == CTRL_CLOSE_EVENT {
            // Windows ends the process when this returns; give main a moment to unhook.
            std::thread::sleep(Duration::from_secs(2));
        }
        TRUE
    }

    pub fn run(args: Args) {
        // SAFETY: registering a handler that only touches an atomic.
        if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) } {
            eprintln!("can't handle Ctrl+C ({e}); close the window to stop");
        }
        let options = HookOptions {
            excluded_apps: args.exclude.clone(),
        };
        if args.demo {
            println!("unflub demo: teh->the, adn->and, recieve->receive. Backspace right after a fix undoes it.");
            drive(DemoCorrector::default(), options);
        } else {
            let backend = match WindowsSpellBackend::new(None) {
                Ok(b) => {
                    println!("Windows spell checker: {}", b.language());
                    Some(Box::new(b) as Box<dyn SpellBackend>)
                }
                Err(e) => {
                    eprintln!("Windows spell checker unavailable ({e}); continuing without it");
                    None
                }
            };
            println!("unflub: engine ready. Type anywhere; Backspace right after a fix undoes it. Ctrl+C to stop.");
            let config = Config {
                excluded_apps: args.exclude,
                ..Config::default()
            };
            drive(Engine::new(config, backend), options);
        }
    }

    fn drive<C: Corrector + Send + 'static>(corrector: C, options: HookOptions) {
        let (hook, events) = match Hook::start(corrector, options) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("failed to install the keyboard hook: {e}");
                std::process::exit(1);
            }
        };
        println!("hooked. Type anywhere; Ctrl+C here to stop.");
        loop {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => print_event(&event),
                Err(RecvTimeoutError::Timeout) if STOP.load(Ordering::SeqCst) => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let report = hook.stop();
        while let Ok(event) = events.try_recv() {
            print_event(&event);
        }
        println!("unhooked. keyboard callback latency: {}", report.keyboard);
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1e3
    }

    fn print_event(event: &HookEvent) {
        match event {
            HookEvent::Corrected {
                replacement: r,
                app,
                took,
            } => println!(
                "fixed  {:?} -> {:?}  ({:?}, {}, {:.3} ms)",
                r.original,
                r.corrected,
                r.source,
                app.as_deref().unwrap_or("?"),
                ms(*took)
            ),
            HookEvent::Undone {
                replacement: r,
                app,
                took,
            } => println!(
                "undo   {:?} -> {:?}  ({}, {:.3} ms)",
                r.corrected,
                r.original,
                app.as_deref().unwrap_or("?"),
                ms(*took)
            ),
            HookEvent::Skipped {
                replacement: r,
                reason,
            } => println!("skip   {:?} -> {:?}  ({reason})", r.original, r.corrected),
            HookEvent::InjectFailed {
                replacement: r,
                sent,
                total,
                released,
            } => eprintln!(
                "fail   {:?} -> {:?}  (SendInput sent {sent}/{total} events{})",
                r.original,
                r.corrected,
                if *released {
                    ", released a held key"
                } else {
                    ""
                }
            ),
            HookEvent::CorrectorPanicked(msg) => {
                eprintln!("the corrector panicked ({msg}); keys now pass through untouched")
            }
            HookEvent::Warning(w) => eprintln!("warning: {w}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags() {
        assert_eq!(parse(&[]), Ok(Some(Args::default())));
        assert_eq!(
            parse(&["--demo", "--exclude", "Code.exe", "--exclude", "x.exe"]),
            Ok(Some(Args {
                demo: true,
                exclude: vec!["code.exe".into(), "x.exe".into()],
            }))
        );
        assert_eq!(parse(&["--help"]), Ok(None));
        assert!(parse(&["--exclude"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
