//! Is a thread's message pump still turning? (#184)
//!
//! The hook thread runs its own Win32 message loop: every keyboard and mouse callback, the
//! watchdog and the injector's results arrive through it. If it blocks, Windows stops calling
//! our hooks (and may drop them), yet the engine worker on another thread keeps answering the
//! app's heartbeat. [`PumpProbe`] posts a private thread message carrying a nonce; the pump
//! stores the nonce in a shared [`PumpAck`] when it gets there ([`answer`]). The probe never
//! blocks, keeps at most one request outstanding and carries no input or window data.
//!
//! A posted thread message wakes `GetMessageW` whether or not anyone types, so an idle pump
//! answers within milliseconds; after a sleep the queued request is answered as soon as the
//! thread runs again.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, MSG, WM_APP};

/// The probe's thread message, `wParam` = nonce. Well away from the hook thread's own
/// `WM_APP + n` messages.
pub const WM_APP_PUMP_PING: u32 = WM_APP + 0x40;

/// What the pump last acknowledged.
#[derive(Debug, Default)]
pub struct PumpAck(AtomicUsize);

impl PumpAck {
    fn acknowledged(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

/// For the pump's loop: acknowledge `msg` if it is a probe. Returns true if it was.
pub fn answer(msg: &MSG, ack: &PumpAck) -> bool {
    if msg.message != WM_APP_PUMP_PING || !msg.hwnd.is_invalid() {
        return false;
    }
    ack.0.store(msg.wParam.0, Ordering::Release);
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpHealth {
    Alive,
    /// The last request went unanswered for longer than the deadline, or couldn't be posted
    /// (the thread or its queue is gone).
    Unresponsive,
}

/// Probes one thread's pump. Call [`PumpProbe::check`] now and then (the engine does it on each
/// heartbeat from the app).
#[derive(Debug)]
pub struct PumpProbe {
    thread_id: u32,
    ack: Arc<PumpAck>,
    deadline: Duration,
    nonce: usize,
    /// When the outstanding request was posted.
    pending_since: Option<Instant>,
}

impl PumpProbe {
    /// Probe the pump of `thread_id` (which must have a message queue), which answers into
    /// `ack`. A request unanswered for `deadline` makes the pump unresponsive.
    pub fn new(thread_id: u32, ack: Arc<PumpAck>, deadline: Duration) -> Self {
        Self {
            thread_id,
            ack,
            deadline,
            nonce: 0,
            pending_since: None,
        }
    }

    /// How the pump stands at `now`. Posts a new request when the last one was answered;
    /// never more than one at a time. A request still within its deadline counts as alive.
    pub fn check(&mut self, now: Instant) -> PumpHealth {
        if let Some(since) = self.pending_since {
            if self.ack.acknowledged() != self.nonce {
                return if now.saturating_duration_since(since) >= self.deadline {
                    PumpHealth::Unresponsive
                } else {
                    PumpHealth::Alive
                };
            }
            self.pending_since = None;
        }
        self.nonce = self.nonce.wrapping_add(1).max(1);
        // SAFETY: posting a plain number to a thread; no pointers cross.
        let posted = unsafe {
            PostThreadMessageW(
                self.thread_id,
                WM_APP_PUMP_PING,
                WPARAM(self.nonce),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            return PumpHealth::Unresponsive;
        }
        self.pending_since = Some(now);
        PumpHealth::Alive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, PeekMessageW, PostThreadMessageW, PM_NOREMOVE, WM_QUIT,
    };

    const DEADLINE: Duration = Duration::from_millis(500);

    /// A private message pump like the hook thread's, without any hook. It waits for `go`
    /// before pumping (a frozen pump until then), counts the probes it answers and ends on
    /// `WM_QUIT`.
    struct TestPump {
        thread_id: u32,
        ack: Arc<PumpAck>,
        go: mpsc::Sender<()>,
        thread: thread::JoinHandle<usize>,
    }

    impl TestPump {
        fn start() -> Self {
            let ack = Arc::new(PumpAck::default());
            let (id_tx, id_rx) = mpsc::channel();
            let (go, go_rx) = mpsc::channel::<()>();
            let thread = thread::spawn({
                let ack = ack.clone();
                move || {
                    let mut msg = MSG::default();
                    // SAFETY: creates this thread's queue, then a plain message loop.
                    unsafe {
                        let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
                        id_tx.send(GetCurrentThreadId()).unwrap();
                        let _ = go_rx.recv();
                        let mut answered = 0;
                        while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
                            if answer(&msg, &ack) {
                                answered += 1;
                            }
                        }
                        answered
                    }
                }
            });
            Self {
                thread_id: id_rx.recv().unwrap(),
                ack,
                go,
                thread,
            }
        }

        fn probe(&self) -> PumpProbe {
            PumpProbe::new(self.thread_id, self.ack.clone(), DEADLINE)
        }

        fn run(&self) {
            self.go.send(()).unwrap();
        }

        fn stop(self) -> usize {
            let _ = self.go.send(());
            // SAFETY: posting to a thread with a queue.
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }.unwrap();
            self.thread.join().unwrap()
        }
    }

    /// Wait until the pump has answered the outstanding request.
    fn settle(pump: &TestPump, probe: &PumpProbe) {
        let started = Instant::now();
        while pump.ack.acknowledged() != probe.nonce {
            assert!(started.elapsed() < Duration::from_secs(5), "no answer");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_turning_pump_answers_and_stays_alive_while_idle() {
        let pump = TestPump::start();
        pump.run();
        let mut probe = pump.probe();
        let start = Instant::now();
        assert_eq!(probe.check(start), PumpHealth::Alive);
        settle(&pump, &probe);
        // No input at all in between; much later (an idle session) it is still alive.
        for minutes in [1, 10, 60] {
            let later = start + Duration::from_secs(60 * minutes);
            assert_eq!(probe.check(later), PumpHealth::Alive);
            settle(&pump, &probe);
        }
        assert_eq!(pump.stop(), 4);
    }

    #[test]
    fn a_blocked_pump_times_out_with_one_request_outstanding() {
        let pump = TestPump::start();
        let mut probe = pump.probe();
        let start = Instant::now();
        assert_eq!(probe.check(start), PumpHealth::Alive);
        // Within the deadline nothing is decided yet and nothing more is posted.
        assert_eq!(probe.check(start + DEADLINE / 2), PumpHealth::Alive);
        assert_eq!(probe.check(start + DEADLINE), PumpHealth::Unresponsive);
        assert_eq!(probe.check(start + DEADLINE * 4), PumpHealth::Unresponsive);
        // Once it turns again it answers the queued request and is alive.
        pump.run();
        settle(&pump, &probe);
        assert_eq!(probe.check(start + DEADLINE * 5), PumpHealth::Alive);
        assert_eq!(pump.stop(), 2, "one request outstanding at a time");
    }

    /// Resume from sleep: the pump didn't run for a long while (here: it was frozen), then
    /// answers the request that waited in its queue, and the next check is alive again.
    #[test]
    fn a_pump_that_answers_late_recovers() {
        let pump = TestPump::start();
        let mut probe = pump.probe();
        let start = Instant::now();
        assert_eq!(probe.check(start), PumpHealth::Alive);
        pump.run();
        settle(&pump, &probe);
        let after_sleep = start + Duration::from_secs(8 * 3600);
        assert_eq!(probe.check(after_sleep), PumpHealth::Alive);
        settle(&pump, &probe);
        assert_eq!(probe.check(after_sleep + DEADLINE * 2), PumpHealth::Alive);
        assert_eq!(pump.stop(), 3);
    }

    /// A probe that can't be posted (no such thread; a stopped thread's id may be reused, so
    /// the test uses 0, never a thread id) counts as unresponsive.
    #[test]
    fn a_probe_that_cannot_be_posted_is_unresponsive() {
        let mut probe = PumpProbe::new(0, Arc::default(), DEADLINE);
        assert_eq!(probe.check(Instant::now()), PumpHealth::Unresponsive);
    }

    #[test]
    fn other_messages_are_not_answers() {
        let ack = PumpAck::default();
        let msg = MSG {
            message: WM_APP_PUMP_PING + 1,
            wParam: WPARAM(7),
            ..Default::default()
        };
        assert!(!answer(&msg, &ack));
        let window = MSG {
            hwnd: HWND(1 as _),
            ..MSG {
                message: WM_APP_PUMP_PING,
                wParam: WPARAM(7),
                ..Default::default()
            }
        };
        assert!(!answer(&window, &ack), "only thread messages");
        assert_eq!(ack.acknowledged(), 0);
    }
}
