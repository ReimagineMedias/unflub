//! Live settings for an [`Engine`] running on the hook thread.
//!
//! [`Hook::start`](crate::Hook::start) moves the corrector onto the hook thread, so nothing
//! else may touch it. [`LiveEngine`] wraps the engine with an inbox: [`LiveConfig::set`] queues
//! a new [`Config`] from any thread, and the hook thread applies it at the start of the next
//! key event (or reset), before that key is fed.
//!
//! Why this is safe on the hook path: the inbox is an unbounded `std::sync::mpsc` channel.
//! Checking it with `try_recv` is a lock-free atomic read when it's empty, and it never waits.
//! Only the newest queued config is applied. Its lookups (excluded apps, personal words, rules)
//! are built by [`LiveConfig::set`] on the caller's thread, and the old ones go back over a
//! second channel to be freed there at the next `set`: with a large personal dictionary either
//! takes milliseconds (about 4 ms for 20,000 words), more than the hook's whole budget.
//!
//! The engine also reports words the user taught it by undoing a fix ([`Event::Learned`]).
//! `LiveEngine` hands each one to a callback right away. The callback runs on the hook thread,
//! so it must not block: send the word to a channel and do the work elsewhere.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use unflub_core::{Action, Config, Context, Corrector, Engine, Event, Key, PreparedConfig};

/// An [`Engine`] whose config can be replaced from another thread. See the module docs.
pub struct LiveEngine {
    engine: Engine,
    configs: Receiver<PreparedConfig>,
    /// Replaced configs, freed by [`LiveConfig`] off the hook thread.
    trash: Sender<PreparedConfig>,
    on_learned: Box<dyn FnMut(String) + Send>,
}

/// Queues configs for a [`LiveEngine`]. Cheap to clone; `set` never waits for the hook.
#[derive(Clone)]
pub struct LiveConfig {
    configs: Sender<PreparedConfig>,
    trash: Arc<Mutex<Receiver<PreparedConfig>>>,
}

impl LiveConfig {
    /// Build `config`'s lookups here and queue it for the next key event. Frees configs the
    /// hook thread replaced since the last call. False once the engine is gone (hook stopped).
    pub fn set(&self, config: Config) -> bool {
        self.empty_trash();
        self.configs.send(PreparedConfig::new(config)).is_ok()
    }

    fn empty_trash(&self) {
        if let Ok(trash) = self.trash.lock() {
            while trash.try_recv().is_ok() {}
        }
    }
}

impl LiveEngine {
    /// Wrap `engine`. `on_learned` gets each word learned by undo, on the hook thread: it must
    /// not block.
    pub fn new(
        engine: Engine,
        on_learned: impl FnMut(String) + Send + 'static,
    ) -> (LiveEngine, LiveConfig) {
        let (tx, rx) = mpsc::channel();
        let (trash_tx, trash_rx) = mpsc::channel();
        (
            LiveEngine {
                engine,
                configs: rx,
                trash: trash_tx,
                on_learned: Box::new(on_learned),
            },
            LiveConfig {
                configs: tx,
                trash: Arc::new(Mutex::new(trash_rx)),
            },
        )
    }

    /// Apply the newest queued config, if any. Never blocks, builds or frees lookups: older
    /// queued configs and the replaced one go back to be freed off this thread.
    fn take_config(&mut self) {
        let mut newest = None;
        for config in self.configs.try_iter() {
            if let Some(older) = newest.replace(config) {
                self.discard(older);
            }
        }
        if let Some(config) = newest {
            let old = self.engine.set_prepared(config);
            self.discard(old);
        }
    }

    fn discard(&self, config: PreparedConfig) {
        if let Err(mpsc::SendError(config)) = self.trash.send(config) {
            // Nobody to free it (the settings side is gone): free it here.
            drop(config);
        }
    }
}

impl Corrector for LiveEngine {
    fn on_key(&mut self, key: Key, ctx: &Context<'_>) -> Action {
        self.take_config();
        let action = self.engine.on_key(key, ctx);
        // The engine only queues events when it acts, so draining here keeps its buffer empty.
        if action != Action::Pass {
            for event in self.engine.drain_events() {
                if let Event::Learned(word) = event {
                    (self.on_learned)(word);
                }
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::TryRecvError;
    use unflub_core::EngineMode;

    fn ctx() -> Context<'static> {
        Context {
            app: Some("notepad.exe"),
            secure: false,
        }
    }

    fn type_text(c: &mut impl Corrector, text: &str) -> Vec<Action> {
        text.chars()
            .map(|ch| {
                let key = if ch.is_alphanumeric() {
                    Key::Char(ch)
                } else {
                    Key::Boundary(ch)
                };
                c.on_key(key, &ctx())
            })
            .collect()
    }

    fn replaced(actions: &[Action]) -> bool {
        actions.iter().any(|a| matches!(a, Action::Replace(_)))
    }

    /// The bundled engine only, so the tests don't depend on an OS spell checker.
    fn bundled() -> Config {
        Config {
            mode: EngineMode::BundledOnly,
            ..Config::default()
        }
    }

    fn rules(rules: &[(&str, &str)]) -> Config {
        Config {
            user_rules: rules
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            ..bundled()
        }
    }

    #[test]
    fn queued_config_applies_on_the_next_key() {
        let (mut live, config) = LiveEngine::new(Engine::new(bundled(), None), |_| {});
        // The first word after a start or reset is never corrected; type one first.
        assert!(!replaced(&type_text(&mut live, " hello ")));

        assert!(config.set(rules(&[("omw", "on my way")])));
        let actions = type_text(&mut live, "omw ");
        assert!(
            matches!(actions.last(), Some(Action::Replace(r)) if r.corrected == "on my way"),
            "{actions:?}"
        );
    }

    #[test]
    fn only_the_newest_queued_config_counts() {
        let (mut live, config) = LiveEngine::new(Engine::new(bundled(), None), |_| {});
        type_text(&mut live, " ");
        config.set(rules(&[("omw", "on my way")]));
        config.set(Config {
            enabled: false,
            ..rules(&[("omw", "on my way")])
        });
        assert!(!replaced(&type_text(&mut live, "omw ")));
        assert!(matches!(live.configs.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn undo_reports_the_learned_word() {
        let (tx, rx) = mpsc::channel();
        let (mut live, _config) = LiveEngine::new(
            Engine::new(rules(&[("omw", "on my way")]), None),
            move |w| {
                let _ = tx.send(w);
            },
        );
        // A user rule fix never teaches; check the bundled dictionary's fix instead.
        let actions = type_text(&mut live, " teh ");
        let Some(Action::Replace(_)) = actions.last() else {
            panic!("expected a fix for teh: {actions:?}");
        };
        assert!(matches!(
            live.on_key(Key::Backspace, &ctx()),
            Action::Undo(_)
        ));
        assert_eq!(rx.try_recv().as_deref(), Ok("teh"));
        assert!(rx.try_recv().is_err());
    }

    /// Review nit 8: the hook thread neither builds nor frees a config's lookups.
    #[test]
    fn replaced_configs_are_freed_off_the_hook_thread() {
        let (mut live, config) = LiveEngine::new(Engine::new(bundled(), None), |_| {});
        type_text(&mut live, " ");
        config.set(rules(&[("omw", "on my way")]));
        config.set(rules(&[("brb", "be right back")]));
        type_text(&mut live, "x ");
        // The skipped config and the replaced one wait to be freed by the settings side.
        assert_eq!(config.trash.lock().unwrap().try_iter().count(), 2);
        config.set(bundled());
        type_text(&mut live, "y ");
        config.set(bundled());
        assert_eq!(
            config.trash.lock().unwrap().try_iter().count(),
            0,
            "set() frees what the hook thread handed back"
        );
        // The newest config was the one applied.
        let actions = type_text(&mut live, "omw ");
        assert!(!replaced(&actions));
    }

    #[test]
    fn set_reports_a_stopped_engine() {
        let (live, config) = LiveEngine::new(Engine::new(bundled(), None), |_| {});
        drop(live);
        assert!(!config.set(Config::default()));
    }
}
