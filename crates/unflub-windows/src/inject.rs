//! Text injection: turn an [`Action`](unflub_core::Action)'s `delete` + `insert` into keystrokes,
//! and keep the user's own keys in order around it.
//!
//! [`plan`], [`with_keys`], [`key_events`] and [`Pipeline`] are pure and tested on every OS. A
//! plan's characters become real keys of the focused app's layout where it has one
//! ([`with_keys`]), with Shift pressed and released around them ([`key_events`]); on Windows,
//! `to_inputs` turns those events into one `INPUT` array for a single `SendInput` call.
//! Every event we send carries a [`Tag`] in `dwExtraInfo` so our own hook recognises it.
//!
//! `SendInput` never runs inside the hook callback: each injected event goes through every
//! low-level hook on the system, ours included, so calling it from the callback stalls input
//! for tens of milliseconds and risks Windows removing the hook. The hook swallows the
//! triggering key and hands the batch to an injector thread instead. Until the batch has come
//! back through the hook, the user's next keys are held in a small queue and then replayed
//! behind it ([`Pipeline`]), so fast typing can't land in the middle of a correction.

use crate::policy::SwallowTracker;

/// One keystroke to send (down then up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stroke {
    /// A real virtual key with its scan code (Backspace, Enter, Tab), sent with Shift as the
    /// user has it.
    Vk { vk: u16, scan: u16 },
    /// One UTF-16 code unit via `KEYEVENTF_UNICODE`. Characters outside the BMP become two.
    Unit(u16),
    /// A character typed with the key that makes it on the focused app's keyboard layout, with
    /// Shift down (`shift`) or up ([`with_keys`]).
    Key { vk: u16, scan: u16, shift: bool },
}

/// Finds the key that types a character on the focused app's keyboard layout.
pub trait KeyLookup {
    /// `(vk, scan, shift)` of a key that types exactly `c` with Shift down (`shift`) or up and
    /// nothing else held, and isn't a dead key; `None` if there is none.
    fn key_for(&self, c: char) -> Option<(u16, u16, bool)>;
}

/// Type every character the layout has a key for with that key, and keep Unicode packets for
/// the rest (characters that need AltGr or a dead key, or have no key at all).
///
/// The Windows 11 Notepad (11.26xx) turns `KEYEVENTF_UNICODE` packets into spaces (#33): the
/// Backspaces landed and the fixed word came out as blanks. Real keys are what every app is
/// built for, so they come first everywhere.
///
/// While the user holds Shift (`held`), only characters typed with Shift become keys; the rest
/// stay packets, so the batch never touches Shift. Releasing and pressing it again around them
/// turns the user's own Shift key-up into a lone Shift tap, and two of those in a row open
/// JetBrains' Search Everywhere (Shift+Space after a typo did it). Vetoing those fixes instead
/// would drop every fix ended by `?`, `!` or `:`, which are typed with Shift held.
pub fn with_keys(strokes: Vec<Stroke>, keys: &impl KeyLookup, held: ShiftKeys) -> Vec<Stroke> {
    strokes
        .into_iter()
        .map(|s| match s {
            // Surrogates aren't chars, so astral characters stay packets.
            Stroke::Unit(u) => char::from_u32(u32::from(u))
                .and_then(|c| keys.key_for(c))
                .filter(|&(_, _, shift)| shift || !held.any())
                .map_or(s, |(vk, scan, shift)| Stroke::Key { vk, scan, shift }),
            other => other,
        })
        .collect()
}

/// Which Shift keys are down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShiftKeys {
    pub left: bool,
    pub right: bool,
}

impl ShiftKeys {
    fn any(self) -> bool {
        self.left || self.right
    }
}

/// One key event to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Vk { vk: u16, scan: u16, up: bool },
    Unit { unit: u16, up: bool },
}

pub const LSHIFT: (u16, u16) = (0xA0, 0x2A);
pub const RSHIFT: (u16, u16) = (0xA1, 0x36);

fn shift_event((vk, scan): (u16, u16), up: bool) -> KeyEvent {
    KeyEvent::Vk { vk, scan, up }
}

/// Down and up events for every stroke, in order, with Shift pressed or released around
/// [`Stroke::Key`]s as each needs, starting from the Shift keys the user holds (`held`) and
/// ending with exactly those down again, so the user's own Shift key-up still matches.
/// Backspace, Enter and Tab go with Shift as the user has it (as they always did); Unicode
/// packets don't care.
pub fn key_events(strokes: &[Stroke], held: ShiftKeys) -> Vec<KeyEvent> {
    let mut out = Vec::with_capacity(strokes.len() * 2 + 4);
    let mut now = held;
    for s in strokes {
        let (want, vk, scan) = match *s {
            Stroke::Vk { vk, scan } => (Some(held.any()), vk, scan),
            Stroke::Key { vk, scan, shift } => (Some(shift), vk, scan),
            Stroke::Unit(unit) => {
                out.push(KeyEvent::Unit { unit, up: false });
                out.push(KeyEvent::Unit { unit, up: true });
                continue;
            }
        };
        match want {
            Some(true) if !now.any() => {
                out.push(shift_event(LSHIFT, false));
                now.left = true;
            }
            Some(false) if now.any() => {
                if now.left {
                    out.push(shift_event(LSHIFT, true));
                }
                if now.right {
                    out.push(shift_event(RSHIFT, true));
                }
                now = ShiftKeys::default();
            }
            _ => {}
        }
        out.push(KeyEvent::Vk {
            vk,
            scan,
            up: false,
        });
        out.push(KeyEvent::Vk { vk, scan, up: true });
    }
    for (side, was, is) in [
        (LSHIFT, held.left, now.left),
        (RSHIFT, held.right, now.right),
    ] {
        if was != is {
            out.push(shift_event(side, !was));
        }
    }
    out
}

pub const BACKSPACE: Stroke = Stroke::Vk {
    vk: 0x08,
    scan: 0x0E,
};
pub const ENTER: Stroke = Stroke::Vk {
    vk: 0x0D,
    scan: 0x1C,
};
pub const TAB: Stroke = Stroke::Vk {
    vk: 0x09,
    scan: 0x0F,
};

/// A correction's keystrokes: `delete` characters before the caret become `insert`, which ends
/// with the boundary key the user pressed.
///
/// The boundary is typed **first**, then deleted along with the word, then the fixed word and
/// the boundary are typed. Fields with inline autocomplete (Excel, the browser address bar,
/// some combo boxes) have the suggested tail selected after the word; a first Backspace would
/// only remove that selection, leaving one letter behind (`teh` + Space became `tthe `).
/// Typing replaces the selection and closes the suggestion. Two more events, the same text
/// everywhere else.
///
/// Enter and Tab can't go first (they would send or commit what is there). A space instead
/// would let Word-style AutoCorrect change the word before it is deleted (#42), so those type
/// `last_typed`, the word's own last letter, again: a letter replaces the selection too and
/// sets off no app's autocorrect (#108: `teh` + Enter with `ran` suggested became `tthe`).
/// Without a letter to repeat they keep the plain order ([`plan`]).
pub fn plan_correction(delete: usize, insert: &str, last_typed: Option<char>) -> Vec<Stroke> {
    let first = match insert.chars().last() {
        Some(b) if unflub_core::undoable_boundary(b) => Some(b),
        Some(_) => last_typed.filter(|c| c.is_alphanumeric()),
        None => None,
    };
    match first {
        Some(c) => {
            let mut out = plan(0, c.encode_utf8(&mut [0; 4]));
            out.extend(plan(delete + 1, insert));
            out
        }
        None => plan(delete, insert),
    }
}

/// `delete` backspaces, then `insert`. `\n` (or a lone `\r`, or `\r\n`) is sent as Enter and
/// `\t` as Tab, since many apps ignore those characters when they arrive as Unicode packets.
pub fn plan(delete: usize, insert: &str) -> Vec<Stroke> {
    let mut out = Vec::with_capacity(delete + insert.len() + 2);
    out.extend(std::iter::repeat_n(BACKSPACE, delete));
    let mut chars = insert.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(ENTER);
            }
            '\n' => out.push(ENTER),
            '\t' => out.push(TAB),
            _ => {
                let mut buf = [0u16; 2];
                out.extend(c.encode_utf16(&mut buf).iter().map(|&u| Stroke::Unit(u)));
            }
        }
    }
    out
}

/// What happened to one `SendInput`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// Every event went out.
    Complete,
    /// Only `sent` of `total` events went out (0 when blocked, e.g. by UIPI). `released` is
    /// true when keys the sent part left in the wrong state were put back with one more call
    /// ([`unwind_injection`], [`unwind_keys`]).
    Partial {
        sent: usize,
        total: usize,
        released: bool,
    },
}

/// What to send after an injection's `SendInput` stopped after `sent` of `events`, so no key is
/// left down and Shift is back where the user holds it (`held`): a key-up for every key the
/// sent part pressed and didn't release, and the Shift keys pressed or released back. Events
/// don't come in down/up pairs (Shift goes down before a capital and up after it), so pairing
/// by count could release the wrong key or none.
pub fn unwind_injection(events: &[KeyEvent], sent: usize, held: ShiftKeys) -> Vec<KeyEvent> {
    let mut down: Vec<KeyEvent> = Vec::new();
    let mut shift = held;
    for e in &events[..sent.min(events.len())] {
        match *e {
            KeyEvent::Vk { vk, scan, up } if (vk, scan) == LSHIFT => shift.left = !up,
            KeyEvent::Vk { vk, scan, up } if (vk, scan) == RSHIFT => shift.right = !up,
            KeyEvent::Vk { vk, scan, up } => {
                down.retain(|d| !matches!(*d, KeyEvent::Vk { vk: v, .. } if v == vk));
                if !up {
                    down.push(KeyEvent::Vk { vk, scan, up: true });
                }
            }
            KeyEvent::Unit { unit, up } => {
                down.retain(|d| !matches!(*d, KeyEvent::Unit { unit: u, .. } if u == unit));
                if !up {
                    down.push(KeyEvent::Unit { unit, up: true });
                }
            }
        }
    }
    for (side, was, is) in [
        (LSHIFT, held.left, shift.left),
        (RSHIFT, held.right, shift.right),
    ] {
        if was != is {
            down.push(shift_event(side, !was));
        }
    }
    down
}

/// What to send after `SendInput` stopped after `sent` of the user's own keys (a replay or a
/// hand-back): a key-up for every key the sent part pressed and didn't release. Their key-ups
/// were in the part that didn't go out.
pub fn unwind_keys(events: &[RawEvent], sent: usize) -> Vec<RawEvent> {
    let mut down: Vec<RawEvent> = Vec::new();
    for e in &events[..sent.min(events.len())] {
        down.retain(|d| d.vk != e.vk || (e.vk == VK_PACKET_CODE && d.scan != e.scan));
        if !e.up {
            down.push(RawEvent { up: true, ..*e });
        }
    }
    down
}

/// `VK_PACKET`: another program's Unicode packet, its character in the scan code.
const VK_PACKET_CODE: u16 = 0xE7;

/// "UF". Distinct from the fixed tag older builds used (0x554E_464C), so a copy of an older
/// build running alongside is never mistaken for ours.
const TAG_MAGIC: usize = 0x5546;
const BATCH_MASK: u16 = 0x3FFF;

/// Our mark in `dwExtraInfo`: which batch an event belongs to and why we sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// A harmless key-up that only checks our injections come back through our own hook
    /// (see [`Pipeline::degraded`]). The hook swallows it, so no app ever sees it.
    Probe(u16),
    /// Part of a correction or undo.
    Inject(u16),
    /// A user key we held back and sent again. `foreign` keeps the fact that another program
    /// injected it originally.
    Replay { batch: u16, foreign: bool },
}

impl Tag {
    pub fn encode(self) -> usize {
        let (kind, batch) = match self {
            Tag::Probe(b) => (0, b),
            Tag::Inject(b) => (1, b),
            Tag::Replay { batch, foreign } => (if foreign { 3 } else { 2 }, batch),
        };
        TAG_MAGIC << 16 | kind << 14 | usize::from(batch & BATCH_MASK)
    }

    pub fn decode(extra: usize) -> Option<Tag> {
        if extra >> 16 != TAG_MAGIC {
            return None;
        }
        let batch = (extra as u16) & BATCH_MASK;
        match (extra >> 14) & 3 {
            0 => Some(Tag::Probe(batch)),
            1 => Some(Tag::Inject(batch)),
            2 => Some(Tag::Replay {
                batch,
                foreign: false,
            }),
            _ => Some(Tag::Replay {
                batch,
                foreign: true,
            }),
        }
    }
}

/// A keyboard event as the hook saw it, with enough detail to send it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub vk: u16,
    pub scan: u16,
    pub extended: bool,
    pub up: bool,
    /// Injected by another program (not by us).
    pub foreign: bool,
}

/// Where a user key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The keyboard, or another program's injection.
    User,
    /// Our replay of a held-back key, from replay batch `n`.
    Replay(u16),
}

/// What to do with a user key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Handle it now (translate, feed the engine). `repeat` is set for auto-repeat key-downs.
    Process { repeat: bool },
    /// Held back; it will be replayed. Swallow it.
    Queued,
    /// The key-up of a key-down we swallowed. Swallow it.
    Consumed,
}

/// Held-back keys to send again, in order, as replay batch `batch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub batch: u16,
    pub events: Vec<RawEvent>,
}

/// An injection batch is over: `sent` of `total` events went out and every sent one came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    pub batch: u16,
    pub sent: usize,
    pub total: usize,
}

/// Held-back keys before we send them again. Past this we replay what we have and start over.
pub const QUEUE_CAPACITY: usize = 64;

/// What became of the triggering key's key-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerUp {
    /// Not seen yet.
    Waiting,
    /// Seen and taken out of the queue.
    Taken,
    /// The key auto-repeated first, so the next key-up belongs to the replayed repeats.
    NotOurs,
}

#[derive(Debug)]
struct Pending {
    batch: u16,
    total: usize,
    seen: usize,
    sent: Option<usize>,
    trigger: RawEvent,
    trigger_up: TriggerUp,
    /// A first Backspace held for a double-Backspace undo that this (undo) injection spends.
    /// If nothing is sent it goes back to the app ahead of the trigger.
    held: Option<RawEvent>,
}

#[derive(Debug)]
struct Flight {
    batch: u16,
    remaining: usize,
}

/// Keeps user keys in order around our injections.
///
/// While an injection is pending (from the swallowed trigger key until its last event has come
/// back through the hook), user keys are held in a queue, key-downs and key-ups alike, in
/// arrival order. Then they are replayed with a [`Tag::Replay`], come back through the hook,
/// and are handled as normal keys (one may trigger the next correction). User keys that
/// arrive while a replay is in flight are held too, behind it. Nothing is dropped or reordered:
/// a full queue, a timeout or a failed send replays what we have.
///
/// If our own events stop coming back (Windows dropped our hook mid-batch, or another
/// program's low-level hook eats injected input), waiting for them would hold every key
/// forever. So the first time anything is still outstanding after the timeout
/// ([`Pipeline::tick`]), the pipeline gives up on it, replays what it holds once, and turns
/// **degraded**: from then on it never holds a key and the hook makes no corrections, until a
/// probe shows our injections come back again ([`Pipeline::recover`]). No key is ever held
/// longer than one timeout.
#[derive(Debug, Default)]
pub struct Pipeline {
    next_batch: u16,
    pending: Option<Pending>,
    flights: Vec<Flight>,
    queue: Vec<RawEvent>,
    /// How many queued events at the front are replays (older than any queued user key).
    replay_front: usize,
    swallow: SwallowTracker,
    outbox: Vec<Replay>,
    busy_since: Option<std::time::Instant>,
    degraded: bool,
}

impl Pipeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Anything pending, in flight or held back.
    pub fn busy(&self) -> bool {
        self.pending.is_some() || !self.flights.is_empty() || !self.queue.is_empty()
    }

    /// Our injected events stopped coming back: keys pass straight through and nothing may be
    /// injected (no [`Pipeline::begin`]) until [`Pipeline::recover`].
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    /// Our events come back again (a probe returned): hold and inject as usual.
    pub fn recover(&mut self) {
        self.degraded = false;
        self.flights.clear();
        self.busy_since = None;
    }

    /// Stop waiting for anything outstanding, replay what is held once, and turn degraded.
    /// The hook also calls this when its hooks were reinstalled in the middle of a batch.
    pub fn abandon(&mut self) {
        self.claim_trigger_up();
        if let Some(p) = self.pending.take() {
            // The app never saw the trigger's key-down: swallow its key-up when it comes.
            if p.trigger_up == TriggerUp::Waiting {
                self.swallow.swallowed(p.trigger.vk);
            }
        }
        self.degraded = true;
        self.start_replay();
        // Nothing is awaited while degraded.
        self.flights.clear();
        self.busy_since = None;
    }

    fn alloc_batch(&mut self) -> u16 {
        self.next_batch = (self.next_batch + 1) & BATCH_MASK;
        if self.next_batch == PASS_BATCH {
            self.next_batch = 1;
        }
        self.next_batch
    }

    /// A user key arrived. `async_down`: Windows already has this key down (only meaningful
    /// for key-downs; see [`SwallowTracker::on_down`]).
    pub fn on_input(&mut self, ev: RawEvent, origin: Origin, async_down: bool) -> Input {
        let replayed = matches!(origin, Origin::Replay(_));
        if let Origin::Replay(batch) = origin {
            self.count_replay(batch);
        }
        // A replay is older than any user key still held, so only a new injection, or older
        // replays queued again in front (a released Backspace, keys held back by an injection),
        // hold it back. Degraded, nothing is ever held.
        let hold = !self.degraded
            && if replayed {
                self.pending.is_some() || self.replay_front > 0
            } else {
                self.busy()
            };
        if hold {
            self.enqueue(ev, replayed);
            return Input::Queued;
        }
        if ev.up {
            if self.swallow.on_up(ev.vk) {
                Input::Consumed
            } else {
                Input::Process { repeat: false }
            }
        } else {
            Input::Process {
                repeat: self.swallow.on_down(ev.vk, async_down),
            }
        }
    }

    fn count_replay(&mut self, batch: u16) {
        if let Some(i) = self.flights.iter().position(|f| f.batch == batch) {
            self.flights[i].remaining -= 1;
            if self.flights[i].remaining == 0 {
                self.flights.remove(i);
            }
        }
    }

    fn enqueue(&mut self, ev: RawEvent, replayed: bool) {
        if self.queue.len() >= QUEUE_CAPACITY {
            self.flush();
        }
        if replayed {
            self.queue.insert(self.replay_front, ev);
            self.replay_front += 1;
        } else {
            self.queue.push(ev);
        }
    }

    /// A key-down the hook holds back for now (the first Backspace of a double-Backspace
    /// undo): its key-up is swallowed too.
    pub fn hold(&mut self, ev: RawEvent) {
        self.swallow.swallowed(ev.vk);
    }

    /// Send a key held with [`Pipeline::hold`] after all, as a key-down and key-up ahead of
    /// anything held now, so the key that caused the release follows it. Call
    /// [`Pipeline::pump`] afterwards.
    pub fn release_held(&mut self, held: RawEvent) {
        let up = RawEvent { up: true, ..held };
        self.queue.splice(0..0, [held, up]);
        self.replay_front += 2;
    }

    /// The trigger was swallowed and an injection of `total` events is about to be sent.
    /// Returns its batch number for the tag. Only one injection is pending at a time; keys are
    /// only processed when none is.
    pub fn begin(&mut self, trigger: RawEvent, total: usize) -> u16 {
        self.begin_with(trigger, None, total)
    }

    /// [`Pipeline::begin`] for an undo that spends `held`, the first Backspace held with
    /// [`Pipeline::hold`]. If `SendInput` takes none of the undo, both Backspaces go back to the
    /// app in order. If it takes only part, the held key counts as spent: the undo's own deletes
    /// already changed the text, and replaying it as well would delete one more character.
    pub fn begin_undo(&mut self, trigger: RawEvent, held: Option<RawEvent>, total: usize) -> u16 {
        self.begin_with(trigger, held, total)
    }

    fn begin_with(&mut self, trigger: RawEvent, held: Option<RawEvent>, total: usize) -> u16 {
        debug_assert!(self.pending.is_none(), "one injection at a time");
        debug_assert!(!self.degraded, "no injections while degraded");
        let batch = self.alloc_batch();
        self.pending = Some(Pending {
            batch,
            total,
            seen: 0,
            sent: None,
            trigger,
            trigger_up: TriggerUp::Waiting,
            held,
        });
        batch
    }

    /// One of our injected events came back through the hook.
    pub fn on_injected(&mut self, batch: u16) -> Option<Finished> {
        let p = self.pending.as_mut().filter(|p| p.batch == batch)?;
        p.seen += 1;
        (p.seen >= p.sent.unwrap_or(p.total)).then(|| self.finish())
    }

    /// The injector reports how many events `SendInput` took.
    pub fn on_sent(&mut self, batch: u16, sent: usize) -> Option<Finished> {
        let p = self.pending.as_mut().filter(|p| p.batch == batch)?;
        p.sent = Some(sent);
        (p.seen >= sent).then(|| self.finish())
    }

    /// The injector reports a replay batch; returns how many held keys could not be sent.
    pub fn on_replay_sent(&mut self, batch: u16, sent: usize, total: usize) -> usize {
        let lost = total.saturating_sub(sent);
        if let Some(i) = self.flights.iter().position(|f| f.batch == batch) {
            let f = &mut self.flights[i];
            f.remaining = f.remaining.saturating_sub(lost);
            if f.remaining == 0 {
                self.flights.remove(i);
            }
        }
        lost
    }

    /// Find the trigger's key-up among the held keys and take it out: the app never saw the
    /// key-down, so it mustn't see this key-up either.
    fn claim_trigger_up(&mut self) {
        let Some(p) = self.pending.as_mut() else {
            return;
        };
        if p.trigger_up != TriggerUp::Waiting {
            return;
        }
        let vk = p.trigger.vk;
        match self.queue.iter().position(|e| e.vk == vk) {
            Some(i) if self.queue[i].up => {
                self.queue.remove(i);
                if i < self.replay_front {
                    self.replay_front -= 1;
                }
                p.trigger_up = TriggerUp::Taken;
            }
            Some(_) => p.trigger_up = TriggerUp::NotOurs,
            None => {}
        }
    }

    fn finish(&mut self) -> Finished {
        self.claim_trigger_up();
        let p = self
            .pending
            .take()
            .expect("finish with a pending injection");
        let sent = p.sent.unwrap_or(p.total);
        if sent == 0 {
            // Nothing was typed: give the user their key back, ahead of everything held (and
            // the first Backspace of an undo ahead of that).
            let mut back = Vec::with_capacity(4);
            if let Some(held) = p.held {
                back.extend([held, RawEvent { up: true, ..held }]);
            }
            back.push(p.trigger);
            if p.trigger_up == TriggerUp::Taken {
                back.push(RawEvent {
                    up: true,
                    ..p.trigger
                });
            }
            self.replay_front += back.len();
            self.queue.splice(0..0, back);
        } else if p.trigger_up == TriggerUp::Waiting {
            self.swallow.swallowed(p.trigger.vk);
        }
        Finished {
            batch: p.batch,
            sent,
            total: p.total,
        }
    }

    /// Replay everything held now, without waiting for the pending injection or flights. Order
    /// is still right: the injector sends batches in the order they are handed over.
    fn flush(&mut self) {
        self.claim_trigger_up();
        self.start_replay();
    }

    fn start_replay(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.queue);
        self.replay_front = 0;
        let batch = self.alloc_batch();
        self.flights.push(Flight {
            batch,
            remaining: events.len(),
        });
        self.outbox.push(Replay { batch, events });
    }

    /// Call after every hook event and injector report: the replays to send now, in order.
    pub fn pump(&mut self, now: std::time::Instant) -> Vec<Replay> {
        if self.pending.is_none() && self.flights.is_empty() {
            self.start_replay();
        }
        if !self.busy() {
            self.busy_since = None;
        } else if self.busy_since.is_none() {
            self.busy_since = Some(now);
        }
        std::mem::take(&mut self.outbox)
    }

    /// The hook is stopping and its hooks are already removed: everything still owed to the
    /// app, in order, to send once without a tag. Call after the injector has finished (or a
    /// bounded wait for it): replays already decided, then the trigger of an injection that
    /// sent nothing (with an undo's held Backspace ahead of it), then the held-back keys. An
    /// injection whose result is unknown counts as sent, so nothing is typed twice. Key-ups of
    /// key-downs the app never saw are left out, as replaying them would have done.
    pub fn drain(&mut self) -> Vec<RawEvent> {
        if self.pending.as_ref().is_some_and(|p| p.sent == Some(0)) {
            self.finish();
        } else if self.pending.is_some() {
            self.claim_trigger_up();
            if let Some(p) = self.pending.take() {
                if p.trigger_up == TriggerUp::Waiting {
                    self.swallow.swallowed(p.trigger.vk);
                }
            }
        }
        let mut out: Vec<RawEvent> = std::mem::take(&mut self.outbox)
            .into_iter()
            .flat_map(|r| r.events)
            .collect();
        for ev in std::mem::take(&mut self.queue) {
            let keep = if ev.up {
                !self.swallow.on_up(ev.vk)
            } else {
                self.swallow.on_down(ev.vk, false);
                true
            };
            if keep {
                out.push(ev);
            }
        }
        self.replay_front = 0;
        self.flights.clear();
        self.busy_since = None;
        out
    }

    /// Busy for longer than `limit`: our events aren't coming back. Give up on them, replay
    /// what is held, and turn degraded ([`Pipeline::abandon`]). Returns true if it did. Call
    /// [`Pipeline::pump`] afterwards.
    pub fn tick(&mut self, now: std::time::Instant, limit: std::time::Duration) -> bool {
        let stale = !self.degraded
            && self
                .busy_since
                .is_some_and(|since| now.duration_since(since) > limit);
        if stale {
            self.abandon();
        }
        stale
    }
}

#[cfg(windows)]
pub use win::{
    pass_inputs, probe_inputs, replay_inputs, send, to_inputs, unwind_inputs, TargetLayout,
};

/// The batch number no injection ever gets: events tagged `Tag::Inject(PASS_BATCH)` match no
/// pending batch, so the hook lets them through untouched (see [`pass_inputs`]).
pub const PASS_BATCH: u16 = 0;

/// The probe's key: unassigned in Windows, so even a program that sees it ignores it.
pub const PROBE_VK: u16 = 0xE8;

#[cfg(windows)]
mod win {
    use super::{KeyEvent, KeyLookup, RawEvent, SendOutcome, Tag};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MapVirtualKeyExW, SendInput, ToUnicodeEx, VkKeyScanExW, HKL, INPUT, INPUT_0,
        INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CAPITAL, VK_LSHIFT, VK_PACKET,
        VK_SHIFT,
    };

    fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS, tag: usize) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: tag,
                },
            },
        }
    }

    /// The events of [`key_events`](super::key_events), in order, tagged as injection `batch`.
    pub fn to_inputs(events: &[KeyEvent], batch: u16) -> Vec<INPUT> {
        key_inputs(events, Tag::Inject(batch).encode())
    }

    fn key_inputs(events: &[KeyEvent], tag: usize) -> Vec<INPUT> {
        let up = |up: bool| {
            if up {
                KEYEVENTF_KEYUP
            } else {
                KEYBD_EVENT_FLAGS(0)
            }
        };
        events
            .iter()
            .map(|e| match *e {
                KeyEvent::Vk { vk, scan, up: u } => key(vk, scan, up(u), tag),
                KeyEvent::Unit { unit, up: u } => key(0, unit, KEYEVENTF_UNICODE | up(u), tag),
            })
            .collect()
    }

    /// The focused app's keyboard layout, for [`with_keys`](super::with_keys).
    pub struct TargetLayout {
        layout: HKL,
        caps_lock: bool,
    }

    impl TargetLayout {
        /// `layout` of the foreground window's thread; `caps_lock` as it is now.
        pub fn new(layout: HKL, caps_lock: bool) -> Self {
            Self { layout, caps_lock }
        }

        /// The one UTF-16 unit `vk` types with Shift down (`shift`) or up and Caps Lock as it
        /// is, without touching the dead-key state. `None` for dead keys and ligatures.
        fn types(&self, vk: u16, scan: u16, shift: bool) -> Option<u16> {
            let mut state = [0u8; 256];
            if shift {
                state[usize::from(VK_SHIFT.0)] = 0x80;
                state[usize::from(VK_LSHIFT.0)] = 0x80;
            }
            if self.caps_lock {
                state[usize::from(VK_CAPITAL.0)] = 0x01;
            }
            let mut buf = [0u16; 4];
            // SAFETY: valid buffers; flag 0x4 keeps the kernel keyboard state unchanged.
            let n = unsafe {
                ToUnicodeEx(
                    u32::from(vk),
                    u32::from(scan),
                    &state,
                    &mut buf,
                    0x4,
                    self.layout,
                )
            };
            (n == 1).then_some(buf[0])
        }
    }

    impl KeyLookup for TargetLayout {
        fn key_for(&self, c: char) -> Option<(u16, u16, bool)> {
            if c.is_control() {
                return None;
            }
            let unit = u16::try_from(u32::from(c)).ok()?;
            // SAFETY: trivial query.
            let found = unsafe { VkKeyScanExW(unit, self.layout) } as u16;
            if found == 0xFFFF {
                return None;
            }
            let (vk, mods) = (found & 0xFF, found >> 8);
            // Ctrl, Alt (AltGr) or Hankaku: never pressed for the user.
            if mods & !1 != 0 {
                return None;
            }
            // SAFETY: trivial query.
            let scan =
                unsafe { MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_VSC, self.layout) } as u16;
            if scan == 0 {
                return None;
            }
            // Check what the key really types: Caps Lock flips some keys and not others, and a
            // dead key types nothing until the next one.
            let shift = mods & 1 != 0;
            [shift, !shift]
                .into_iter()
                .find(|&s| self.types(vk, scan, s) == Some(unit))
                .map(|s| (vk, scan, s))
        }
    }

    /// One key-up of an unassigned key, tagged as probe `batch`. Our hook swallows it.
    pub fn probe_inputs(batch: u16) -> Vec<INPUT> {
        vec![key(
            super::PROBE_VK,
            0,
            KEYEVENTF_KEYUP,
            Tag::Probe(batch).encode(),
        )]
    }

    /// Held-back user keys, as they were, tagged as replay `batch`.
    pub fn replay_inputs(events: &[RawEvent], batch: u16) -> Vec<INPUT> {
        tagged(events, |e| {
            Tag::Replay {
                batch,
                foreign: e.foreign,
            }
            .encode()
        })
    }

    /// User keys to hand to the app directly: a hook of ours still installed lets them through
    /// without counting or holding them (`Tag::Inject(PASS_BATCH)`).
    pub fn pass_inputs(events: &[RawEvent]) -> Vec<INPUT> {
        tagged(events, |_| Tag::Inject(super::PASS_BATCH).encode())
    }

    fn tagged(events: &[RawEvent], tag: impl Fn(&RawEvent) -> usize) -> Vec<INPUT> {
        events
            .iter()
            .map(|e| {
                let tag = tag(e);
                let up = if e.up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                };
                if e.vk == VK_PACKET.0 {
                    // Another program's Unicode packet: the character is in the scan code.
                    return key(0, e.scan, KEYEVENTF_UNICODE | up, tag);
                }
                let ext = if e.extended {
                    KEYEVENTF_EXTENDEDKEY
                } else {
                    KEYBD_EVENT_FLAGS(0)
                };
                key(e.vk, e.scan, up | ext, tag)
            })
            .collect()
    }

    fn send_raw(inputs: &[INPUT]) -> usize {
        if inputs.is_empty() {
            return 0;
        }
        // SAFETY: `inputs` is a valid slice of initialised INPUT structs and cbsize matches.
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize }
    }

    /// One `SendInput` call for the whole batch. Anything short of every event is a failure;
    /// `unwind(sent)` gives the events that put keys left down (or Shift left changed) back,
    /// sent with one more call ([`unwind_injection`](super::unwind_injection),
    /// [`unwind_keys`](super::unwind_keys)).
    pub fn send(inputs: &[INPUT], unwind: impl FnOnce(usize) -> Vec<INPUT>) -> SendOutcome {
        let total = inputs.len();
        let sent = send_raw(inputs);
        if sent == total {
            return SendOutcome::Complete;
        }
        let back = unwind(sent);
        let released = !back.is_empty() && send_raw(&back) == back.len();
        SendOutcome::Partial {
            sent,
            total,
            released,
        }
    }

    /// Key events that put things back after a partial send. Tagged as passing through, so our
    /// hook neither counts them toward the batch (they would finish it as if complete) nor
    /// holds them.
    pub fn unwind_inputs(events: &[KeyEvent]) -> Vec<INPUT> {
        key_inputs(events, Tag::Inject(super::PASS_BATCH).encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn deletes_then_types() {
        assert_eq!(
            plan(3, "the "),
            vec![
                BACKSPACE,
                BACKSPACE,
                BACKSPACE,
                Stroke::Unit('t' as u16),
                Stroke::Unit('h' as u16),
                Stroke::Unit('e' as u16),
                Stroke::Unit(' ' as u16),
            ]
        );
    }

    /// Strokes into a field holding `text` with the last `selected` bytes selected (an inline
    /// autocomplete suggestion): typing or a Backspace first removes the selection.
    fn into_field(text: &str, selected: usize, strokes: &[Stroke]) -> String {
        let mut text = text.to_string();
        let mut selection = selected;
        for s in strokes {
            let typed = match *s {
                BACKSPACE if selection > 0 => None,
                BACKSPACE => {
                    text.pop();
                    None
                }
                ENTER => Some('\n'),
                TAB => Some('\t'),
                Stroke::Unit(c) => Some(char::from_u32(u32::from(c)).unwrap()),
                other => panic!("{other:?}"),
            };
            text.truncate(text.len() - selection);
            selection = 0;
            text.extend(typed);
        }
        text
    }

    /// Review S2: inline autocomplete selects a suggested tail after the word. Typing the
    /// boundary first replaces it; then the word and that boundary go.
    #[test]
    fn corrections_type_the_boundary_first() {
        let u = |c: char| Stroke::Unit(c as u16);
        assert_eq!(
            plan_correction(3, "the ", Some('h')),
            vec![
                u(' '),
                BACKSPACE,
                BACKSPACE,
                BACKSPACE,
                BACKSPACE,
                u('t'),
                u('h'),
                u('e'),
                u(' '),
            ]
        );
        assert_eq!(plan_correction(3, "the.", Some('h'))[0], u('.'));
        // Both kinds of field: plain, and one with "ran" selected after "teh".
        for selected in ["", "ran"] {
            let field = format!("ok teh{selected}");
            let plan = plan_correction(3, "the ", Some('h'));
            assert_eq!(
                into_field(&field, selected.len(), &plan),
                "ok the ",
                "{selected:?}"
            );
        }
    }

    /// #108: Enter and Tab would send or commit, so they come last, once; the word's last
    /// letter goes first to clear an autocomplete selection without setting off the app's
    /// own autocorrect (a space would, #42).
    #[test]
    fn enter_and_tab_corrections_type_a_letter_first() {
        let u = |c: char| Stroke::Unit(c as u16);
        for (insert, key) in [("the\n", ENTER), ("the\t", TAB)] {
            let plan = plan_correction(3, insert, Some('h'));
            assert_eq!(plan[0], u('h'));
            assert_eq!(plan[1..5], [BACKSPACE; 4]);
            assert_eq!(plan.iter().filter(|s| **s == key).count(), 1);
            assert_eq!(plan.last(), Some(&key));
            for selected in ["", "ran"] {
                let field = format!("ok teh{selected}");
                let want = format!("ok {insert}");
                assert_eq!(
                    into_field(&field, selected.len(), &plan),
                    want,
                    "{selected:?}"
                );
            }
        }
        // The plain order (which leaves "tthe" behind a selection) only without a letter.
        assert_eq!(into_field("ok tehran", 3, &plan(3, "the\n")), "ok tthe\n");
        assert_eq!(plan_correction(3, "the\n", None), plan(3, "the\n"));
        assert_eq!(plan_correction(3, "the\n", Some('.')), plan(3, "the\n"));
        assert!(plan_correction(0, "", Some('h')).is_empty());
    }

    #[test]
    fn newline_and_tab_are_real_keys() {
        assert_eq!(plan(0, "a\n"), vec![Stroke::Unit('a' as u16), ENTER]);
        assert_eq!(plan(0, "a\t"), vec![Stroke::Unit('a' as u16), TAB]);
        assert_eq!(plan(0, "\r\n"), vec![ENTER]);
        assert_eq!(plan(0, "\r"), vec![ENTER]);
    }

    #[test]
    fn astral_characters_become_surrogate_pairs() {
        assert_eq!(
            plan(1, "\u{1F600}é"),
            vec![
                BACKSPACE,
                Stroke::Unit(0xD83D),
                Stroke::Unit(0xDE00),
                Stroke::Unit(0x00E9),
            ]
        );
    }

    /// A US layout: letters, digits, space and the shifted punctuation of the top row.
    struct Us;

    const US_SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const US_PLAIN: &str = "`1234567890-=[]\\;',./";

    impl KeyLookup for Us {
        fn key_for(&self, c: char) -> Option<(u16, u16, bool)> {
            let shifted = |i: usize| (0x100 + i as u16, i as u16 + 1);
            match c {
                'a'..='z' => Some((c.to_ascii_uppercase() as u16, c as u16, false)),
                'A'..='Z' => Some((c as u16, c as u16, true)),
                '0'..='9' | ' ' => Some((c as u16, c as u16, false)),
                _ => US_SHIFTED
                    .find(c)
                    .map(|i| (shifted(i).0, shifted(i).1, true))
                    .or_else(|| US_PLAIN.find(c).map(|i| (0x200 + i as u16, 0x80, false))),
            }
        }
    }

    /// A text field that gets key events: Shift as the events leave it, Backspace deletes, keys
    /// type what [`Us`] says they type.
    fn type_into(text: &str, held: ShiftKeys, events: &[KeyEvent]) -> (String, ShiftKeys) {
        let mut text = text.to_string();
        let mut shift = held;
        for e in events {
            match *e {
                KeyEvent::Vk { vk, scan, up } => {
                    if (vk, scan) == LSHIFT {
                        shift.left = !up;
                    } else if (vk, scan) == RSHIFT {
                        shift.right = !up;
                    } else if up {
                    } else if vk == 0x08 {
                        text.pop();
                    } else if vk == 0x0D {
                        text.push('\n');
                    } else {
                        let typed = (0..=0x2FFFu32)
                            .filter_map(char::from_u32)
                            .find(|&c| Us.key_for(c) == Some((vk, scan, shift.any())))
                            .unwrap_or_else(|| panic!("{vk:#x} types nothing (shift {shift:?})"));
                        text.push(typed);
                    }
                }
                KeyEvent::Unit { unit, up: false } => {
                    text.push(char::from_u32(unit.into()).unwrap())
                }
                KeyEvent::Unit { .. } => {}
            }
        }
        (text, shift)
    }

    /// #33: characters go out as their keys, so apps that turn Unicode packets into spaces
    /// (the Windows 11 Notepad) get the real word.
    #[test]
    fn characters_with_a_key_are_typed_as_keys() {
        let strokes = with_keys(plan(3, "The é?\u{1F600}"), &Us, ShiftKeys::default());
        assert_eq!(&strokes[..3], &[BACKSPACE; 3]);
        assert_eq!(
            strokes[3],
            Stroke::Key {
                vk: 'T' as u16,
                scan: 'T' as u16,
                shift: true
            }
        );
        assert_eq!(
            strokes[4],
            Stroke::Key {
                vk: 'H' as u16,
                scan: 'h' as u16,
                shift: false
            }
        );
        assert!(matches!(strokes[6], Stroke::Key { vk: 0x20, .. }));
        assert_eq!(strokes[7], Stroke::Unit(0xE9), "no key for é: a packet");
        assert!(matches!(strokes[8], Stroke::Key { shift: true, .. }), "?");
        assert_eq!(
            &strokes[9..],
            &[Stroke::Unit(0xD83D), Stroke::Unit(0xDE00)],
            "astral characters stay packets"
        );
    }

    /// The real lookup on this thread's layout (a query; nothing is typed). Space is the same
    /// key on every layout; control characters and astral ones never get a key.
    #[cfg(windows)]
    #[test]
    fn the_windows_layout_finds_keys() {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
        // SAFETY: trivial query.
        let layout = unsafe { GetKeyboardLayout(0) };
        for caps in [false, true] {
            let keys = TargetLayout::new(layout, caps);
            assert_eq!(keys.key_for(' '), Some((0x20, 0x39, false)), "caps {caps}");
            assert_eq!(keys.key_for('\n'), None);
            assert_eq!(keys.key_for('\u{1F600}'), None);
        }
    }

    #[test]
    fn shift_is_pressed_for_capitals_and_left_as_the_user_holds_it() {
        let none = ShiftKeys::default();
        // No Shift needed, none held: just the keys.
        let events = key_events(
            &with_keys(plan_correction(3, "the ", Some('h')), &Us, none),
            none,
        );
        assert_eq!(events.len(), 2 * 9);
        assert_eq!(type_into("ok teh", none, &events), ("ok the ".into(), none));

        // A capital: one Shift press around it.
        let events = key_events(&with_keys(plan(3, "The "), &Us, none), none);
        assert_eq!(events[6], shift_event(LSHIFT, false));
        assert_eq!(events[9], shift_event(LSHIFT, true));
        assert_eq!(type_into("Teh", none, &events), ("The ".into(), none));

        // "teh?" or "teh " typed with Shift still held (review finding 2): Shift is never
        // touched. The '?' is its key, the letters and the space go as packets, so the user's
        // own Shift key-up can't become a lone Shift tap (JetBrains' Search Everywhere).
        for held in [
            ShiftKeys {
                left: false,
                right: true,
            },
            ShiftKeys {
                left: true,
                right: false,
            },
            ShiftKeys {
                left: true,
                right: true,
            },
        ] {
            for (insert, want) in [("the?", "so the?"), ("the ", "so the ")] {
                let plan = with_keys(plan_correction(3, insert, Some('h')), &Us, held);
                assert!(
                    plan.iter()
                        .all(|s| !matches!(s, Stroke::Key { shift: false, .. })),
                    "{plan:?}"
                );
                let events = key_events(&plan, held);
                assert_eq!(type_into("so teh", held, &events), (want.into(), held));
                let shift_events = events
                    .iter()
                    .filter(|e| {
                        matches!(e, KeyEvent::Vk { vk, .. } if *vk == LSHIFT.0 || *vk == RSHIFT.0)
                    })
                    .count();
                assert_eq!(shift_events, 0, "{held:?} {insert:?}");
            }
        }

        // Packets don't move Shift.
        let events = key_events(&[Stroke::Unit(0xE9)], none);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn partial_send_releases_a_held_key() {
        // The user's own keys: whatever the sent part pressed is released.
        let keys = [down(N), up(N), down(SHIFT), down(O), up(O), up(SHIFT)];
        assert!(unwind_keys(&keys, 6).is_empty(), "complete");
        assert!(unwind_keys(&keys, 0).is_empty(), "nothing sent");
        assert!(unwind_keys(&keys, 2).is_empty(), "stopped between keys");
        assert_eq!(unwind_keys(&keys, 1), vec![up(N)]);
        assert_eq!(unwind_keys(&keys, 4), vec![up(SHIFT), up(O)]);
        assert_eq!(unwind_keys(&keys, 5), vec![up(SHIFT)]);
    }

    /// Which keys are down after `events`, starting from Shift as `held`.
    fn left_down(held: ShiftKeys, events: &[KeyEvent]) -> (Vec<u16>, ShiftKeys) {
        let mut keys = Vec::new();
        let mut shift = held;
        for e in events {
            match *e {
                KeyEvent::Vk { vk, scan, up } if (vk, scan) == LSHIFT => shift.left = !up,
                KeyEvent::Vk { vk, scan, up } if (vk, scan) == RSHIFT => shift.right = !up,
                KeyEvent::Vk { vk, up, .. } | KeyEvent::Unit { unit: vk, up } => {
                    keys.retain(|k| *k != vk);
                    if !up {
                        keys.push(vk);
                    }
                }
            }
        }
        (keys, shift)
    }

    /// Review finding 1: Shift events aren't paired with a key, so a partial send is unwound
    /// by what is down, not by counting pairs.
    #[test]
    fn a_partial_injection_leaves_no_key_down_and_shift_as_held() {
        let none = ShiftKeys::default();
        let events = key_events(&with_keys(plan(3, "The"), &Us, none), none);
        // BS x3 (6 events), then LShift down: SendInput stopped there.
        assert_eq!(events[6], shift_event(LSHIFT, false));
        let back = unwind_injection(&events, 7, none);
        assert_eq!(back, vec![shift_event(LSHIFT, true)]);
        let mut sent = events[..7].to_vec();
        sent.extend(&back);
        assert_eq!(left_down(none, &sent), (vec![], none), "nothing stuck");
        assert_eq!(
            type_into("ok teh", none, &sent).0,
            "ok ",
            "nothing typed beyond the sent part"
        );

        // Every stopping point, with and without Shift held.
        let right = ShiftKeys {
            left: false,
            right: true,
        };
        for (held, plan) in [
            (none, plan_correction(3, "The ", Some('h'))),
            (right, plan_correction(3, "the?", Some('h'))),
        ] {
            let events = key_events(&with_keys(plan, &Us, held), held);
            for stop in 0..=events.len() {
                let mut sent = events[..stop].to_vec();
                sent.extend(unwind_injection(&events, stop, held));
                assert_eq!(left_down(held, &sent), (vec![], held), "{held:?} {stop}");
            }
        }
    }

    #[test]
    fn empty_action_plans_nothing() {
        assert!(plan(0, "").is_empty());
    }

    #[test]
    fn tags_round_trip_and_reject_foreign_extra_info() {
        for tag in [
            Tag::Inject(0),
            Tag::Inject(BATCH_MASK),
            Tag::Replay {
                batch: 7,
                foreign: false,
            },
            Tag::Replay {
                batch: 7,
                foreign: true,
            },
        ] {
            assert_eq!(Tag::decode(tag.encode()), Some(tag));
        }
        assert_eq!(Tag::decode(0), None);
        assert_eq!(Tag::decode(0x554E_464C), None, "old fixed tag");
        assert_eq!(Tag::decode(0xFFFF_FFFF), None);
        assert_eq!(Tag::decode(TAG_MAGIC << 16), Some(Tag::Probe(0)));
        assert_eq!(Tag::decode(Tag::Probe(9).encode()), Some(Tag::Probe(9)));
    }

    fn down(vk: u16) -> RawEvent {
        RawEvent {
            vk,
            scan: vk,
            extended: false,
            up: false,
            foreign: false,
        }
    }

    fn up(vk: u16) -> RawEvent {
        RawEvent {
            up: true,
            ..down(vk)
        }
    }

    const SPACE: u16 = 0x20;
    const SHIFT: u16 = 0xA0;
    const BACK: u16 = 0x08;
    const N: u16 = 0x4E;
    const O: u16 = 0x4F;
    const COMMA: u16 = 0xBC;

    fn now() -> Instant {
        Instant::now()
    }

    /// Feed a user key; true if it was processed (not held or consumed).
    fn user(p: &mut Pipeline, ev: RawEvent) -> Input {
        p.on_input(ev, Origin::User, false)
    }

    /// Start an injection of `total` events triggered by `trigger`, and report it sent.
    fn inject(p: &mut Pipeline, trigger: RawEvent, total: usize) -> u16 {
        assert_eq!(
            user(p, trigger),
            Input::Process { repeat: false },
            "trigger processed"
        );
        let b = p.begin(trigger, total);
        assert!(p.pump(now()).is_empty());
        assert_eq!(p.on_sent(b, total), None);
        b
    }

    fn come_back(p: &mut Pipeline, batch: u16, total: usize) -> Finished {
        for _ in 1..total {
            assert_eq!(p.on_injected(batch), None);
        }
        p.on_injected(batch).expect("finished on the last event")
    }

    fn replay(p: &mut Pipeline, r: &Replay) -> Vec<Input> {
        r.events
            .iter()
            .map(|e| p.on_input(*e, Origin::Replay(r.batch), false))
            .collect()
    }

    /// Review blocker B1: a batch whose echo never comes back must not hold keys forever.
    #[test]
    fn a_lost_echo_degrades_to_pass_through_after_one_timeout() {
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 8);
        let start = now();
        assert_eq!(user(&mut p, down(N)), Input::Queued);
        // Only half the echo comes back, then nothing.
        for _ in 0..4 {
            assert_eq!(p.on_injected(b), None);
        }
        assert!(!p.tick(start, Duration::from_millis(500)), "not stale yet");
        assert!(p.tick(
            start + Duration::from_millis(600),
            Duration::from_millis(500)
        ));
        assert!(p.degraded());
        // The held key goes out once, and nothing waits for it.
        let r = p.pump(now());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].events, vec![down(N)]);
        assert!(!p.busy());
        // The trigger's key-up is eaten; from now on keys are never held, user or replayed.
        assert_eq!(user(&mut p, up(SPACE)), Input::Consumed);
        for _ in 0..20 {
            assert_eq!(user(&mut p, down(O)), Input::Process { repeat: false });
            assert_eq!(user(&mut p, up(O)), Input::Process { repeat: false });
            let later = now() + Duration::from_secs(5);
            assert!(!p.tick(later, Duration::from_millis(500)));
            assert!(p.pump(now()).is_empty());
        }
        assert_eq!(
            replay(&mut p, &r[0]),
            vec![Input::Process { repeat: false }]
        );
        // A late echo from the abandoned batch changes nothing.
        assert_eq!(p.on_injected(b), None);

        p.recover();
        assert!(!p.degraded());
        let b = inject(&mut p, down(SPACE), 2);
        assert_eq!(user(&mut p, down(N)), Input::Queued, "holding again");
        come_back(&mut p, b, 2);
    }

    /// Replays that never come back degrade the same way.
    #[test]
    fn lost_replays_degrade_too() {
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 2);
        assert_eq!(user(&mut p, down(N)), Input::Queued);
        come_back(&mut p, b, 2);
        let r = p.pump(now());
        assert_eq!(r.len(), 1);
        // The replay never returns; the user keeps typing.
        assert_eq!(user(&mut p, down(O)), Input::Queued);
        let later = now() + Duration::from_millis(600);
        assert!(p.tick(later, Duration::from_millis(500)));
        assert_eq!(p.pump(now())[0].events, vec![down(O)]);
        assert_eq!(user(&mut p, up(O)), Input::Process { repeat: false });
    }

    /// Double-Backspace undo: the held first Backspace goes out before the key that released
    /// it, in order, and its own key-up never reaches the app twice.
    #[test]
    fn a_held_backspace_replays_before_the_next_key() {
        let mut p = Pipeline::new();
        assert_eq!(user(&mut p, down(BACK)), Input::Process { repeat: false });
        p.hold(down(BACK));
        assert_eq!(
            user(&mut p, up(BACK)),
            Input::Consumed,
            "its up is swallowed"
        );
        // A comma arrives: the Backspace goes first, the comma behind it.
        p.release_held(down(BACK));
        assert_eq!(user(&mut p, down(COMMA)), Input::Queued);
        let r = p.pump(now());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].events, vec![down(BACK), up(BACK), down(COMMA)]);
        assert_eq!(
            replay(&mut p, &r[0]),
            vec![Input::Process { repeat: false }; 3]
        );
        assert!(!p.busy());

        // Released by the timer, with nothing behind it.
        let mut p = Pipeline::new();
        user(&mut p, down(BACK));
        p.hold(down(BACK));
        p.release_held(down(BACK));
        assert_eq!(p.pump(now())[0].events, vec![down(BACK), up(BACK)]);
        assert_eq!(
            user(&mut p, up(BACK)),
            Input::Queued,
            "behind the replay in flight"
        );
    }

    /// Review blocker: a replayed key that releases a held Backspace waits behind it, and so
    /// do the rest of its replay batch still on the way.
    #[test]
    fn replays_released_behind_a_held_backspace_stay_in_order() {
        let mut p = Pipeline::new();
        user(&mut p, down(BACK));
        p.hold(down(BACK));
        assert_eq!(user(&mut p, up(BACK)), Input::Consumed);
        // A replay batch (keys held back earlier) arrives: [comma down, comma up, N down].
        let batch = 7;
        p.flights.push(Flight {
            batch,
            remaining: 3,
        });
        // The comma releases the Backspace first (the hook does this for replays too) and is
        // queued behind it instead of reaching the app ahead of it.
        p.release_held(down(BACK));
        assert_eq!(
            p.on_input(down(COMMA), Origin::Replay(batch), false),
            Input::Queued
        );
        // The rest of the batch follows it, not ahead of it.
        assert_eq!(
            p.on_input(up(COMMA), Origin::Replay(batch), false),
            Input::Queued
        );
        assert_eq!(
            p.on_input(down(N), Origin::Replay(batch), false),
            Input::Queued
        );
        let r = p.pump(now());
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].events,
            vec![down(BACK), up(BACK), down(COMMA), up(COMMA), down(N)]
        );
        // They come back as replays and are processed in that order.
        assert_eq!(
            replay(&mut p, &r[0]),
            vec![Input::Process { repeat: false }; 5]
        );
        assert!(!p.busy());
    }

    /// Review finding 4: an undo that sends nothing gives back both Backspaces, held one first.
    #[test]
    fn a_failed_undo_gives_back_both_backspaces() {
        let mut p = Pipeline::new();
        user(&mut p, down(BACK));
        p.hold(down(BACK));
        assert_eq!(user(&mut p, up(BACK)), Input::Consumed);
        assert_eq!(user(&mut p, down(BACK)), Input::Process { repeat: false });
        let b = p.begin_undo(down(BACK), Some(down(BACK)), 10);
        assert_eq!(user(&mut p, up(BACK)), Input::Queued);
        assert_eq!(
            p.on_sent(b, 0),
            Some(Finished {
                batch: b,
                sent: 0,
                total: 10
            })
        );
        let r = p.pump(now());
        assert_eq!(
            r[0].events,
            vec![down(BACK), up(BACK), down(BACK), up(BACK)]
        );

        // A partial undo spends the held key: only nothing extra goes back.
        let mut p = Pipeline::new();
        user(&mut p, down(BACK));
        p.hold(down(BACK));
        user(&mut p, up(BACK));
        user(&mut p, down(BACK));
        let b = p.begin_undo(down(BACK), Some(down(BACK)), 10);
        assert_eq!(p.on_sent(b, 4), None);
        for _ in 0..3 {
            assert_eq!(p.on_injected(b), None);
        }
        assert!(p.on_injected(b).is_some());
        assert!(p.pump(now()).is_empty());
    }

    /// Review finding 6: stopping hands back what is held, in order, without typing twice.
    #[test]
    fn drain_returns_what_is_owed_in_order() {
        // An injection in flight with keys behind it: the injection counts as typed.
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 8);
        for ev in [down(SHIFT), up(SPACE), down(N), up(N), up(SHIFT)] {
            assert_eq!(user(&mut p, ev), Input::Queued);
        }
        let _ = b;
        assert_eq!(
            p.drain(),
            vec![down(SHIFT), down(N), up(N), up(SHIFT)],
            "the trigger's key-up is left out; the Shift release is kept"
        );
        assert!(!p.busy());

        // An injection that sent nothing: its trigger goes first.
        let mut p = Pipeline::new();
        assert_eq!(user(&mut p, down(SPACE)), Input::Process { repeat: false });
        let b = p.begin(down(SPACE), 8);
        assert_eq!(user(&mut p, down(N)), Input::Queued);
        p.pending.as_mut().unwrap().sent = Some(0);
        let _ = b;
        assert_eq!(p.drain(), vec![down(SPACE), down(N)]);

        // A result that never came: assumed typed, never sent twice.
        let mut p = Pipeline::new();
        assert_eq!(user(&mut p, down(SPACE)), Input::Process { repeat: false });
        p.begin(down(SPACE), 8);
        assert_eq!(user(&mut p, down(O)), Input::Queued);
        assert_eq!(p.drain(), vec![down(O)]);
    }

    #[test]
    fn batch_numbers_skip_the_pass_through_batch() {
        let mut p = Pipeline::new();
        p.next_batch = BATCH_MASK;
        assert_eq!(p.alloc_batch(), 1);
        assert_ne!(p.alloc_batch(), PASS_BATCH);
    }

    #[test]
    fn idle_keys_are_processed() {
        let mut p = Pipeline::new();
        assert_eq!(user(&mut p, down(N)), Input::Process { repeat: false });
        assert_eq!(user(&mut p, up(N)), Input::Process { repeat: false });
        assert!(p.pump(now()).is_empty());
        assert!(!p.busy());
    }

    #[test]
    fn keys_during_pending_are_held_then_replayed_in_order() {
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 8);
        // Fast typing while the correction is in flight, including the trigger's own key-up.
        for ev in [down(N), up(SPACE), up(N), down(O)] {
            assert_eq!(user(&mut p, ev), Input::Queued);
        }
        assert!(
            p.pump(now()).is_empty(),
            "nothing replayed before the batch is back"
        );
        assert_eq!(
            come_back(&mut p, b, 8),
            Finished {
                batch: b,
                sent: 8,
                total: 8
            }
        );
        let r = p.pump(now());
        assert_eq!(r.len(), 1);
        // The trigger's key-up is dropped: the app never saw its key-down.
        assert_eq!(r[0].events, vec![down(N), up(N), down(O)]);
        // A key typed while the replay is in flight waits behind it.
        assert_eq!(user(&mut p, up(O)), Input::Queued);
        assert_eq!(
            replay(&mut p, &r[0]),
            vec![Input::Process { repeat: false }; 3]
        );
        let r2 = p.pump(now());
        assert_eq!(r2[0].events, vec![up(O)]);
        replay(&mut p, &r2[0]);
        assert!(p.pump(now()).is_empty());
        assert!(!p.busy());
    }

    #[test]
    fn trigger_key_up_after_the_batch_is_consumed() {
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 2);
        come_back(&mut p, b, 2);
        assert!(p.pump(now()).is_empty());
        assert_eq!(user(&mut p, up(SPACE)), Input::Consumed);
        assert_eq!(user(&mut p, up(SPACE)), Input::Process { repeat: false });
    }

    #[test]
    fn key_up_of_a_key_pressed_before_pending_is_replayed() {
        let mut p = Pipeline::new();
        assert_eq!(user(&mut p, down(SHIFT)), Input::Process { repeat: false });
        let b = inject(&mut p, down(0xBF), 4); // '?' with Shift held
        assert_eq!(user(&mut p, up(SHIFT)), Input::Queued);
        come_back(&mut p, b, 4);
        let r = p.pump(now());
        assert_eq!(r[0].events, vec![up(SHIFT)], "Shift is released, not stuck");
    }

    #[test]
    fn repeats_of_the_trigger_keep_their_key_up() {
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 2);
        // Space held down: repeats, then the release.
        for ev in [down(SPACE), down(SPACE), up(SPACE)] {
            assert_eq!(user(&mut p, ev), Input::Queued);
        }
        come_back(&mut p, b, 2);
        let r = p.pump(now());
        assert_eq!(r[0].events, vec![down(SPACE), down(SPACE), up(SPACE)]);
        // And no later space key-up gets eaten.
        replay(&mut p, &r[0]);
        p.pump(now());
        assert_eq!(user(&mut p, down(SPACE)), Input::Process { repeat: false });
        assert_eq!(user(&mut p, up(SPACE)), Input::Process { repeat: false });
    }

    #[test]
    fn overflow_replays_what_is_held_without_dropping_or_reordering() {
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 2);
        let typed: Vec<RawEvent> = (0..QUEUE_CAPACITY as u16 + 3)
            .map(|i| {
                if i % 2 == 0 {
                    down(0x41 + i % 26)
                } else {
                    up(0x41 + i % 26)
                }
            })
            .collect();
        for ev in &typed {
            assert_eq!(user(&mut p, *ev), Input::Queued);
        }
        let first = p.pump(now());
        assert_eq!(first.len(), 1, "full queue flushed");
        assert_eq!(first[0].events.len(), QUEUE_CAPACITY);
        assert_eq!(p.queue.len(), 3, "the rest is still held");
        come_back(&mut p, b, 2);
        // The flushed replay comes back; the rest waits for it, then follows.
        assert!(p.pump(now()).is_empty());
        replay(&mut p, &first[0]);
        let second = p.pump(now());
        let all: Vec<RawEvent> = first[0]
            .events
            .iter()
            .chain(&second[0].events)
            .copied()
            .collect();
        assert_eq!(all, typed);
    }

    #[test]
    fn failed_send_gives_the_trigger_back_first() {
        let mut p = Pipeline::new();
        assert_eq!(user(&mut p, down(SPACE)), Input::Process { repeat: false });
        let b = p.begin(down(SPACE), 8);
        assert_eq!(user(&mut p, down(N)), Input::Queued);
        assert_eq!(user(&mut p, up(SPACE)), Input::Queued);
        assert_eq!(
            p.on_sent(b, 0),
            Some(Finished {
                batch: b,
                sent: 0,
                total: 8
            })
        );
        let r = p.pump(now());
        assert_eq!(r[0].events, vec![down(SPACE), up(SPACE), down(N)]);
    }

    #[test]
    fn partial_send_finishes_when_the_sent_events_are_back() {
        let mut p = Pipeline::new();
        user(&mut p, down(SPACE));
        let b = p.begin(down(SPACE), 8);
        assert_eq!(p.on_injected(b), None);
        assert_eq!(p.on_injected(b), None);
        assert_eq!(p.on_sent(b, 3), None, "event 3 hasn't come back yet");
        assert_eq!(
            p.on_injected(b),
            Some(Finished {
                batch: b,
                sent: 3,
                total: 8
            })
        );
    }

    #[test]
    fn events_can_come_back_before_the_send_report() {
        let mut p = Pipeline::new();
        user(&mut p, down(SPACE));
        let b = p.begin(down(SPACE), 2);
        assert_eq!(p.on_injected(b), None);
        assert!(p.on_injected(b).is_some());
        assert_eq!(p.on_sent(b, 2), None, "already finished");
    }

    #[test]
    fn undo_during_pending_is_replayed_and_can_inject() {
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 8);
        // Backspace right after the fix, before the fix has landed.
        assert_eq!(user(&mut p, down(BACK)), Input::Queued);
        assert_eq!(user(&mut p, up(BACK)), Input::Queued);
        come_back(&mut p, b, 8);
        let r = p.pump(now());
        assert_eq!(r[0].events, vec![down(BACK), up(BACK)]);
        // The replayed Backspace is processed and triggers the undo.
        assert_eq!(
            p.on_input(down(BACK), Origin::Replay(r[0].batch), false),
            Input::Process { repeat: false }
        );
        let undo = p.begin(down(BACK), 10);
        // Its replayed key-up is still in flight: held behind the undo, then consumed.
        assert_eq!(
            p.on_input(up(BACK), Origin::Replay(r[0].batch), false),
            Input::Queued
        );
        p.on_sent(undo, 10);
        come_back(&mut p, undo, 10);
        assert!(p.pump(now()).is_empty(), "the trigger's key-up was dropped");
        assert!(!p.busy());
    }

    #[test]
    fn replays_still_in_flight_stay_ahead_of_newer_keys() {
        const P: u16 = 0x50;
        let mut p = Pipeline::new();
        let b = inject(&mut p, down(SPACE), 2);
        for ev in [up(SPACE), down(N), up(N), down(O), up(O)] {
            assert_eq!(user(&mut p, ev), Input::Queued);
        }
        come_back(&mut p, b, 2);
        let r = p.pump(now());
        assert_eq!(r[0].events, vec![down(N), up(N), down(O), up(O)]);
        let from_replay = |p: &mut Pipeline, ev| p.on_input(ev, Origin::Replay(r[0].batch), false);
        assert_eq!(
            from_replay(&mut p, down(N)),
            Input::Process { repeat: false }
        );
        assert_eq!(from_replay(&mut p, up(N)), Input::Process { repeat: false });
        // A newer key typed while the replay is in flight waits.
        assert_eq!(user(&mut p, down(P)), Input::Queued);
        // The replayed "o" triggers another fix; its replayed key-up is still in flight and
        // goes in front of the newer "p".
        assert_eq!(
            from_replay(&mut p, down(O)),
            Input::Process { repeat: false }
        );
        let b2 = p.begin(down(O), 2);
        assert_eq!(from_replay(&mut p, up(O)), Input::Queued);
        assert_eq!(p.queue, vec![up(O), down(P)]);
        p.on_sent(b2, 2);
        come_back(&mut p, b2, 2);
        let r2 = p.pump(now());
        assert_eq!(r2[0].events, vec![down(P)], "trigger o's key-up taken out");
    }

    #[test]
    fn stale_batches_are_flushed_after_the_limit() {
        let mut p = Pipeline::new();
        let start = now();
        user(&mut p, down(SPACE));
        p.begin(down(SPACE), 2);
        assert_eq!(user(&mut p, down(N)), Input::Queued);
        assert!(p.pump(start).is_empty());
        let limit = Duration::from_millis(500);
        assert!(!p.tick(start + Duration::from_millis(100), limit));
        assert!(p.tick(start + Duration::from_millis(600), limit));
        let r = p.pump(start + Duration::from_millis(600));
        assert_eq!(
            r[0].events,
            vec![down(N)],
            "held keys go out even if the batch never returns"
        );
    }
}
