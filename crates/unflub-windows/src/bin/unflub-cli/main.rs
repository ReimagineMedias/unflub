//! `unflub-cli`: headless runner for the Windows adapter.
//!
//! ```text
//! unflub-cli --demo [--exclude <exe>]... [--pill <style>]   three fixed rules (teh, adn, recieve); try it now
//! unflub-cli [--exclude <exe>]... [--pill <style>]          the real engine
//! unflub-cli --dry-run --pill off                           hook and engine, but never inject or swallow a key
//! ```
//!
//! Prints each correction and undo with the hook callback time, and shows the correction pill
//! at the caret. Ctrl+C unhooks, prints the hook latency histogram and exits.

#[cfg_attr(not(windows), allow(dead_code))]
mod demo;

use unflub_windows::PillStyle;

const USAGE: &str = "usage: unflub-cli [--demo] [--dry-run] [--exclude <exe>]... [--pill <style>]

  --demo           use three built-in rules (teh->the, adn->and, recieve->receive)
                   instead of the engine
  --dry-run        install the hook and run the engine, but never inject input or
                   swallow a key: fixes are only printed as skipped. If the hook
                   can't be installed, keep the engine loaded anyway (for
                   measuring its footprint)
  --exclude <exe>  never correct in this app (e.g. code.exe); repeatable
  --pill <style>   how fixes are shown at the caret: underline-capsule (default),
                   capsule, underline or off
  -h, --help       show this help";

#[derive(Debug, Default, PartialEq)]
struct Args {
    demo: bool,
    dry_run: bool,
    exclude: Vec<String>,
    pill: PillStyle,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => out.demo = true,
            "--dry-run" => out.dry_run = true,
            "--exclude" => {
                let exe = args.next().ok_or("--exclude needs an exe name")?;
                out.exclude.push(exe.to_lowercase());
            }
            "--pill" => {
                out.pill = args.next().ok_or("--pill needs a style")?.parse()?;
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
    use unflub_windows::{
        Correction, Corrector, Hook, HookEvent, HookOptions, InjectTiming, Overlay, OverlayHandle,
        PillConfig, PillStyle, WindowsSpellBackend,
    };
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
            dry_run: args.dry_run,
        };
        if args.dry_run {
            println!("dry run: nothing will be injected and no key will be swallowed.");
        }
        let overlay = start_overlay(args.pill);
        if args.demo {
            println!("unflub demo: teh->the, adn->and, recieve->receive. Backspace right after a fix undoes it.");
            drive(DemoCorrector::default(), options, overlay);
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
            drive(Engine::new(config, backend), options, overlay);
        }
    }

    fn start_overlay(style: PillStyle) -> Option<OverlayHandle> {
        if style == PillStyle::Off {
            return None;
        }
        match Overlay::spawn(PillConfig {
            style,
            ..PillConfig::default()
        }) {
            Ok(o) => Some(o),
            Err(e) => {
                eprintln!("correction pill unavailable ({e}); continuing without it");
                None
            }
        }
    }

    fn drive<C: Corrector + Send + 'static>(
        corrector: C,
        options: HookOptions,
        overlay: Option<OverlayHandle>,
    ) {
        let dry_run = options.dry_run;
        let (hook, events) = match Hook::start(corrector, options) {
            Ok(h) => h,
            Err(e) if dry_run => {
                // The dictionary index is a process-wide static, so it stays loaded: what is
                // left is the engine-only footprint, which is what a perf check can still measure.
                println!("hook unavailable: {e}. Dry run continues engine-only; Ctrl+C to stop.");
                while !STOP.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                return;
            }
            Err(e) => {
                eprintln!("failed to install the keyboard hook: {e}");
                std::process::exit(1);
            }
        };
        println!("hooked. Type anywhere; Ctrl+C here to stop.");
        loop {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    // The hook already sent this event; the pill costs the hook nothing.
                    if let (Some(o), Some(c)) = (&overlay, Correction::from_hook_event(&event)) {
                        o.show(c);
                    }
                    print_event(&event);
                }
                Err(RecvTimeoutError::Timeout) if STOP.load(Ordering::SeqCst) => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let report = hook.stop();
        if let Some(o) = overlay {
            o.shutdown();
        }
        while let Ok(event) = events.try_recv() {
            print_event(&event);
        }
        println!("unhooked. keyboard callback latency: {}", report.keyboard);
        println!("  decide:  {}", report.decide);
        println!("  inject:  {}", report.inject);
        println!("  landed:  {}", report.end_to_end);
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1e3
    }

    fn times(hook: Duration, t: &InjectTiming) -> String {
        format!(
            "hook {:.3} ms, decide {:.3} ms, inject {:.3} ms, landed {:.3} ms",
            ms(hook),
            ms(t.decide),
            ms(t.inject),
            ms(t.end_to_end)
        )
    }

    fn print_event(event: &HookEvent) {
        match event {
            HookEvent::Corrected {
                replacement: r,
                app,
                took,
                timing,
            } => println!(
                "fixed  {:?} -> {:?}  ({:?}, {}, {})",
                r.original,
                r.corrected,
                r.source,
                app.as_deref().unwrap_or("?"),
                times(*took, timing)
            ),
            HookEvent::Undone {
                replacement: r,
                app,
                took,
                timing,
            } => println!(
                "undo   {:?} -> {:?}  ({}, {})",
                r.corrected,
                r.original,
                app.as_deref().unwrap_or("?"),
                times(*took, timing)
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
            HookEvent::HooksReinstalled { reason } => eprintln!("hooks reinstalled ({reason})"),
            HookEvent::UndoPending { replacement: r } => {
                println!(
                    "undo?  {:?} -> {:?}  (Backspace again to undo)",
                    r.corrected, r.original
                )
            }
            HookEvent::UndoDeclined => println!("undo?  no: the held Backspace went on"),
            HookEvent::InputInterference { active: true } => eprintln!(
                "our injected input isn't coming back: corrections paused, keys pass through"
            ),
            HookEvent::InputInterference { active: false } => {
                eprintln!("our injected input comes back again: corrections resumed")
            }
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
                dry_run: false,
                exclude: vec!["code.exe".into(), "x.exe".into()],
                pill: PillStyle::UnderlineCapsule,
            }))
        );
        assert_eq!(
            parse(&["--pill", "capsule"]).map(|a| a.map(|a| a.pill)),
            Ok(Some(PillStyle::Capsule))
        );
        assert!(parse(&["--pill"]).is_err());
        assert!(parse(&["--pill", "sparkles"]).is_err());
        assert_eq!(
            parse(&["--dry-run", "--pill", "off"]),
            Ok(Some(Args {
                dry_run: true,
                pill: PillStyle::Off,
                ..Args::default()
            }))
        );
        assert_eq!(parse(&["--help"]), Ok(None));
        assert!(parse(&["--exclude"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
