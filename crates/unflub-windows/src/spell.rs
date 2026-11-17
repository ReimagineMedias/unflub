//! [`SpellBackend`] over the Windows spell checker (`ISpellCheckerFactory` / `ISpellChecker`).
//!
//! COM threading: the spell checker factory is registered `ThreadingModel=Free`, so its
//! objects live in the multithreaded apartment (MTA) and may be called directly from any MTA
//! thread. [`WindowsSpellBackend::new`] joins the calling thread to the MTA and fails if that
//! thread is already an STA. The hook thread also joins the MTA, so the engine can call the
//! backend there with no marshalling. The background lookup thread owns its own checker.
//!
//! Speed: `Check` on a correct word is ~0.04 ms, fine for the hook path. `ComprehensiveCheck`
//! on a misspelling takes 5-100 ms, so [`SpellBackend::auto_replacement`] only reads a cache
//! and queues misses for the background thread; the answer is used the next time the word
//! comes up.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use unflub_core::SpellBackend;
use windows::core::{Result, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_OK};
use windows::Win32::Globalization::{
    GetUserDefaultUILanguage, ISpellChecker, ISpellCheckerFactory, LCIDToLocaleName,
    SpellCheckerFactory, CORRECTIVE_ACTION_REPLACE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

const FALLBACK_LANGUAGE: &str = "en-US";
/// Cache entries before the cache is cleared. Words, not bytes: a few hundred KB at most.
const CACHE_LIMIT: usize = 4096;
/// Pending background lookups. Extra requests are dropped (and retried next time).
const QUEUE_LIMIT: usize = 64;

/// Work for the `unflub-spell` thread: everything slow or touching disk.
enum Job {
    /// `ComprehensiveCheck` the word and cache the answer.
    Lookup(String),
    /// `ISpellChecker::Add` (writes the user's dictionary file).
    Learn(String),
}

#[derive(Default)]
struct Cache {
    answers: HashMap<String, Option<String>>,
    pending: HashSet<String>,
}

pub struct WindowsSpellBackend {
    checker: ISpellChecker,
    language: String,
    cache: Arc<Mutex<Cache>>,
    jobs: Sender<Job>,
}

// SAFETY: the checker is a free-threaded COM object created in the MTA (see module docs).
// It may be used from any thread that is in the MTA. `new` joins its thread to the MTA and
// never leaves it, so the process MTA lives as long as the process. The hook thread joins the
// MTA explicitly; if that ever failed, the hook thread (which then has made no COM init call)
// is still in the implicit MTA that exists because of `new`, so calls stay direct and legal.
// A thread that joined an STA must not use this backend; nothing in unflub does.
unsafe impl Send for WindowsSpellBackend {}

/// Join the calling thread to the MTA. The matching CoUninitialize is deliberately never
/// called: the MTA must outlive every COM object handed out, and lives as long as the process.
pub fn ensure_mta() -> Result<()> {
    // SAFETY: CoInitializeEx with no reserved pointer.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr == RPC_E_CHANGED_MODE {
        return Err(hr.into());
    }
    hr.ok()
}

fn factory() -> Result<ISpellCheckerFactory> {
    // SAFETY: COM is initialised (callers run ensure_mta first).
    unsafe { CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER) }
}

fn supported(factory: &ISpellCheckerFactory, tag: &str) -> bool {
    // SAFETY: valid interface and string.
    unsafe { factory.IsSupported(&HSTRING::from(tag)) }.is_ok_and(|b| b.as_bool())
}

/// The user's UI language as a BCP-47 tag (e.g. `en-GB`), if Windows can name it.
fn ui_language() -> Option<String> {
    // SAFETY: buffer sized to LOCALE_NAME_MAX_LENGTH (85).
    unsafe {
        let langid = GetUserDefaultUILanguage();
        let mut buf = [0u16; 85];
        let n = LCIDToLocaleName(u32::from(langid), Some(&mut buf), 0);
        (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
    }
}

/// Call `f` with `word` as a NUL-terminated wide string, on the stack for ordinary words so
/// the per-word hook path doesn't allocate.
fn with_pcwstr<R>(word: &str, f: impl FnOnce(PCWSTR) -> R) -> R {
    let mut buf = [0u16; 64];
    for (i, unit) in word.encode_utf16().enumerate() {
        if i == buf.len() - 1 {
            let heap = HSTRING::from(word);
            return f(PCWSTR(heap.as_ptr()));
        }
        buf[i] = unit;
    }
    f(PCWSTR(buf.as_ptr()))
}

/// Take ownership of a CoTaskMem string.
fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: p is a valid NUL-terminated string allocated with CoTaskMemAlloc; freed once.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// The OS's own "safe to replace" fix for `word`, if it has one. Slow (5-100 ms): background only.
fn comprehensive_replacement(checker: &ISpellChecker, word: &str) -> Option<String> {
    let len = word.encode_utf16().count() as u32;
    // SAFETY: valid interface; errors enumerated and released by their wrappers.
    unsafe {
        let errors = checker.ComprehensiveCheck(&HSTRING::from(word)).ok()?;
        let mut err = None;
        if errors.Next(&mut err) != S_OK {
            return None;
        }
        let err = err?;
        let whole_word = err.StartIndex().ok()? == 0 && err.Length().ok()? == len;
        if !whole_word || err.CorrectiveAction().ok()? != CORRECTIVE_ACTION_REPLACE {
            return None;
        }
        let replacement = take_pwstr(err.Replacement().ok()?);
        (!replacement.is_empty()).then_some(replacement)
    }
}

impl WindowsSpellBackend {
    /// Create a checker for `language` (a BCP-47 tag such as `en-US`), or by default the
    /// user's UI language, falling back to `en-US` when Windows has no dictionary for it.
    /// Joins the calling thread to the MTA; fails on an STA thread.
    pub fn new(language: Option<&str>) -> Result<Self> {
        ensure_mta()?;
        let factory = factory()?;
        let language = match language {
            Some(tag) => tag.to_owned(),
            None => ui_language()
                .filter(|tag| supported(&factory, tag))
                .unwrap_or_else(|| FALLBACK_LANGUAGE.to_owned()),
        };
        // SAFETY: valid interface and string.
        let checker = unsafe { factory.CreateSpellChecker(&HSTRING::from(language.as_str())) }?;
        let cache = Arc::new(Mutex::new(Cache::default()));
        let (tx, rx) = mpsc::channel();
        let worker_cache = Arc::clone(&cache);
        let worker_language = language.clone();
        std::thread::Builder::new()
            .name("unflub-spell".into())
            .spawn(move || lookup_worker(&worker_language, rx, worker_cache))
            .map_err(|e| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string())
            })?;
        Ok(Self {
            checker,
            language,
            cache,
            jobs: tx,
        })
    }

    /// The BCP-47 tag in use.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// A handle that teaches this checker words from any thread, through the same background
    /// thread as [`SpellBackend::learn`]. Use it where the backend itself is out of reach
    /// (it lives inside the engine on the hook thread).
    pub fn learner(&self) -> SpellLearner {
        SpellLearner {
            jobs: self.jobs.clone(),
        }
    }

    /// Blocking `ComprehensiveCheck` lookup, bypassing the cache. Slow; never call it on the
    /// hook path. For diagnostics and tests.
    pub fn auto_replacement_blocking(&self, word: &str) -> Option<String> {
        comprehensive_replacement(&self.checker, word)
    }
}

/// Adds words to the user's Windows dictionary on the `unflub-spell` thread. Never blocks.
/// Keeps that thread alive while any clone exists.
#[derive(Clone)]
pub struct SpellLearner {
    jobs: Sender<Job>,
}

impl SpellLearner {
    pub fn learn(&self, word: &str) {
        let _ = self.jobs.send(Job::Learn(word.to_owned()));
    }
}

fn lookup_worker(language: &str, rx: Receiver<Job>, cache: Arc<Mutex<Cache>>) {
    let checker = ensure_mta().and_then(|()| {
        // SAFETY: COM initialised on this thread just above.
        unsafe { factory()?.CreateSpellChecker(&HSTRING::from(language)) }
    });
    while let Ok(job) = rx.recv() {
        let word = match job {
            Job::Lookup(word) => word,
            Job::Learn(word) => {
                if let Ok(c) = &checker {
                    // SAFETY: valid interface on this MTA thread.
                    let _ = unsafe { c.Add(&HSTRING::from(word.as_str())) };
                }
                if let Ok(mut cache) = cache.lock() {
                    cache.answers.remove(&word);
                }
                continue;
            }
        };
        let answer = checker
            .as_ref()
            .ok()
            .and_then(|c| comprehensive_replacement(c, &word));
        let Ok(mut cache) = cache.lock() else { return };
        cache.pending.remove(&word);
        if cache.answers.len() >= CACHE_LIMIT {
            cache.answers.clear();
        }
        cache.answers.insert(word, answer);
    }
}

impl SpellBackend for WindowsSpellBackend {
    /// `Check`: fast. Errors count as "correct" so a failing checker never causes corrections.
    fn is_correct(&self, word: &str) -> bool {
        with_pcwstr(word, |text| {
            // SAFETY: valid interface used from an MTA thread; `text` outlives the call.
            unsafe {
                let Ok(errors) = self.checker.Check(text) else {
                    return true;
                };
                let mut err = None;
                !(errors.Next(&mut err) == S_OK && err.is_some())
            }
        })
    }

    /// Cache hit only; a miss queues a background lookup and returns `None`.
    fn auto_replacement(&self, word: &str) -> Option<String> {
        let Ok(mut cache) = self.cache.lock() else {
            return None;
        };
        if let Some(answer) = cache.answers.get(word) {
            return answer.clone();
        }
        if cache.pending.len() < QUEUE_LIMIT
            && !cache.pending.contains(word)
            && self.jobs.send(Job::Lookup(word.to_owned())).is_ok()
        {
            cache.pending.insert(word.to_owned());
        }
        None
    }

    /// `ISpellChecker::Add` adds to the user's Windows dictionary for this language. That
    /// writes a file, so it runs on the background thread, never on the hook thread.
    fn learn(&self, word: &str) {
        let _ = self.jobs.send(Job::Learn(word.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn backend() -> Option<WindowsSpellBackend> {
        match WindowsSpellBackend::new(Some("en-US")) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("skipping: no en-US Windows spell checker ({e})");
                None
            }
        }
    }

    #[test]
    fn checks_real_words_and_typos() {
        let Some(b) = backend() else { return };
        assert_eq!(b.language(), "en-US");
        for w in ["the", "receive", "because", "keyboard", "Ottawa"] {
            let t = Instant::now();
            assert!(b.is_correct(w), "{w} should be correct");
            eprintln!("Check({w}) = correct in {:?}", t.elapsed());
        }
        for w in ["teh", "recieve", "becuase"] {
            let t = Instant::now();
            assert!(!b.is_correct(w), "{w} should be flagged");
            eprintln!("Check({w}) = flagged in {:?}", t.elapsed());
        }
    }

    #[test]
    fn auto_replacement_is_cached_and_filled_in_the_background() {
        let Some(b) = backend() else { return };
        // Windows' own auto-replace list (measured on Windows 11 26300): it has entries for
        // recieve and becuase but not teh. If a fix is offered it must be the right one.
        let expected = [
            ("teh", "the"),
            ("recieve", "receive"),
            ("becuase", "because"),
            ("the", ""),
        ];
        for (w, fix) in expected {
            let t = Instant::now();
            let blocking = b.auto_replacement_blocking(w);
            eprintln!(
                "ComprehensiveCheck({w}) -> {blocking:?} in {:?}",
                t.elapsed()
            );
            if let Some(got) = &blocking {
                assert_eq!(got, fix, "auto-replacement for {w}");
            }

            let t = Instant::now();
            assert_eq!(b.auto_replacement(w), None, "first call is a cache miss");
            assert!(
                t.elapsed() < Duration::from_millis(5),
                "miss must not block"
            );

            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if b.cache.lock().unwrap().answers.contains_key(w) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "background lookup for {w} timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(b.auto_replacement(w), blocking, "cached answer matches");
        }
        assert_eq!(b.auto_replacement_blocking("the"), None);
    }

    #[test]
    fn stack_and_heap_paths_agree() {
        let Some(b) = backend() else { return };
        // Windows treats words of 64+ UTF-16 units as correct; either way both paths must
        // give the same answer as a plain HSTRING call, on both sides of the 63-unit buffer.
        for w in ["teh", "the"]
            .map(String::from)
            .into_iter()
            .chain([10, 62, 63, 64, 100].map(|n| "q".repeat(n)))
        {
            // SAFETY: valid interface on an MTA thread.
            let direct = unsafe {
                let errors = b.checker.Check(&HSTRING::from(w.as_str())).unwrap();
                let mut err = None;
                !(errors.Next(&mut err) == S_OK && err.is_some())
            };
            assert_eq!(b.is_correct(&w), direct, "{} units", w.len());
        }
        assert!(!b.is_correct(&"q".repeat(63)));
    }
}
