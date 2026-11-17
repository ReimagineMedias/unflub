//! Live settings for an [`Engine`] running on the hook thread.
//!
//! [`Hook::start`](crate::Hook::start) moves the corrector onto the hook thread, so nothing
//! else may touch it. [`LiveEngine`] wraps the engine with an inbox: [`LiveConfig::set`] queues
//! a new [`Config`] from any thread, and the hook thread applies it at the start of the next
//! key event (or reset), before that key is fed.
//!
//! Why this is safe on the hook path: the inbox is an unbounded `std::sync::mpsc` channel.
//! Checking it with `try_recv` is a lock-free atomic read when it's empty, and it never waits.
//! Only the newest queued config is applied. `Engine::set_config` rebuilds a few small hash
//! sets (excluded apps, personal words, rules): microseconds, once per settings change.
//!
//! The engine also reports words the user taught it by undoing a fix ([`Event::Learned`]).
//! `LiveEngine` hands each one to a callback right away. The callback runs on the hook thread,
//! so it must not block: send the word to a channel and do the work elsewhere.

use std::sync::mpsc::{self, Receiver, Sender};

use unflub_core::{Action, Config, Context, Corrector, Engine, Event, Key};

/// An [`Engine`] whose config can be replaced from another thread. See the module docs.
pub struct LiveEngine {
    engine: Engine,
    configs: Receiver<Config>,
    on_learned: Box<dyn FnMut(String) + Send>,
}

/// Queues configs for a [`LiveEngine`]. Cheap to clone; every method returns immediately.
#[derive(Clone)]
pub struct LiveConfig {
    configs: Sender<Config>,
}

impl LiveConfig {
    /// Queue `config` for the next key event. False once the engine is gone (hook stopped).
    pub fn set(&self, config: Config) -> bool {
        self.configs.send(config).is_ok()
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
        (
            LiveEngine {
                engine,
                configs: rx,
                on_learned: Box::new(on_learned),
            },
            LiveConfig { configs: tx },
        )
    }

    /// Apply the newest queued config, if any. Never blocks.
    fn take_config(&mut self) {
        if let Some(config) = self.configs.try_iter().last() {
            self.engine.set_config(config);
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

    #[test]
    fn set_reports_a_stopped_engine() {
        let (live, config) = LiveEngine::new(Engine::new(bundled(), None), |_| {});
        drop(live);
        assert!(!config.set(Config::default()));
    }
}
