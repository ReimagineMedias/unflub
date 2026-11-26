//! Noisy-channel ranking of dictionary candidates for a typed word.
//!
//! score(c) = ln P(c | previous word) - edit_weight * cost(typed -> c)
//!
//! A correction wins only if it beats every rival by a margin: other candidates, the typed word
//! itself when it is a known word, and the hypothesis that an unknown word was meant.

use crate::dict::Dictionary;
use crate::distance::{self, MAX_LEN};
use crate::layout::Adjacency;

/// Candidates costing more than this are never considered...
const MAX_COST: f32 = 2.0;
/// ...and short words allow less: this much per typed letter.
const MAX_COST_PER_LETTER: f32 = 0.35;
/// Weight of the bigram estimate when the previous word is known.
const BIGRAM_WEIGHT: f32 = 0.5;
/// Extra edit cost for a candidate that drops the plural or past ending the user typed
/// (`jumsp` to `jump`, `happend` to `happens`): a slip rarely adds a whole inflection.
const DROPPED_INFLECTION: f32 = 0.5;
/// From this many typed letters such a candidate is never taken: a long word's inflected form
/// is often missing from the list (`counterrevolutionarys`), and its stem is the wrong word.
const NO_DROPPED_INFLECTION_FROM: usize = 12;

/// Whether `cand` loses an `-s` or `-ed` that `typed` has. Plural: the candidate doesn't end in
/// `s` but the typed word does (`returs`, not `-ss`), or has it second to last and the
/// candidate is shorter (`jumsp`).
/// Past: typed ends in `ed` (or a consonant and `d`, five letters or more: `happend`) and the
/// candidate doesn't end in `d`.
fn drops_inflection(typed: &[u8], cand: &[u8]) -> bool {
    let n = typed.len();
    if n < 4 {
        return false;
    }
    let tail = &typed[n - 2..];
    let plural = cand.last() != Some(&b's')
        && match tail {
            [b's', b's'] => false,
            [_, b's'] => true,
            [b's', _] => cand.len() < n,
            _ => false,
        };
    let consonant_d = n >= 5 && tail[1] == b'd' && !b"aeiouy".contains(&tail[0]);
    let past = (tail == b"ed" || consonant_d) && cand.last() != Some(&b'd');
    plural || past
}

#[derive(Debug, Clone, Copy)]
pub struct Tuning {
    /// Nats per unit of edit cost.
    pub edit_weight: f32,
    /// Log probability that an unknown four-letter typed word is intentional (a name, jargon,
    /// a new word).
    pub oov_log_p: f32,
    /// How much less likely that is per extra letter: short unknown words are often
    /// abbreviations, long ones usually typos.
    pub oov_per_letter: f32,
}

/// Fitted on the owner's typo corpus, a held-out list of common typos and a list of jargon and
/// names that must stay unchanged.
pub const TUNING: Tuning = Tuning {
    edit_weight: 6.5,
    oov_log_p: -17.0,
    oov_per_letter: 3.0,
};

pub struct Choice {
    pub id: u32,
    /// Score lead over the runner-up (another candidate, the typed word itself, or "leave it").
    pub margin: f32,
}

pub struct Ranker {
    pub tuning: Tuning,
    adj: Adjacency,
    scratch: Vec<u32>,
}

impl Ranker {
    pub fn new(adj: Adjacency) -> Self {
        Self {
            tuning: TUNING,
            adj,
            scratch: Vec::with_capacity(4096),
        }
    }

    fn context_log_p(dict: &Dictionary, id: u32, prev: Option<u32>) -> f32 {
        match prev {
            None => dict.log_p(id),
            Some(p) => {
                let uni = dict.log_p(id).exp();
                let bi = dict.bigram(p, id).unwrap_or(0.0);
                (BIGRAM_WEIGHT * bi + (1.0 - BIGRAM_WEIGHT) * uni).ln()
            }
        }
    }

    /// Best correction for `typed` (lowercase ASCII). A known typed word always competes as
    /// itself. `leave_alone`, if set, adds the hypothesis that an unknown word was typed on
    /// purpose, made more likely by the given number of nats (for example for a capitalised
    /// word mid-sentence, which is probably a name).
    pub fn best(
        &mut self,
        dict: &Dictionary,
        typed: &[u8],
        prev: Option<u32>,
        leave_alone: Option<f32>,
    ) -> Option<Choice> {
        if typed.len() > MAX_LEN {
            return None;
        }
        let own = dict.id(typed);
        let rival = match own {
            Some(own) => Self::context_log_p(dict, own, prev),
            None => match leave_alone {
                Some(bonus) => {
                    let extra = typed.len() as f32 - 4.0;
                    let context = if prev.is_some() {
                        (1.0 - BIGRAM_WEIGHT).ln()
                    } else {
                        0.0
                    };
                    self.tuning.oov_log_p - self.tuning.oov_per_letter * extra + context + bonus
                }
                None => f32::NEG_INFINITY,
            },
        };
        let limit =
            (typed.len() as f32 * MAX_COST_PER_LETTER).min(MAX_COST) + 2.0 * distance::EDIT_BASE;
        let weight = self.tuning.edit_weight;
        dict.candidates(typed, &mut self.scratch);
        let mut best: Option<(u32, f32)> = None;
        let mut second = f32::NEG_INFINITY;
        // Ids run from frequent to rare, so the bar rises quickly and most rare candidates are
        // dismissed on frequency alone, before computing a distance.
        for &id in &self.scratch {
            if Some(id) == own {
                continue;
            }
            let word = dict.word(id).as_bytes();
            if word.len().abs_diff(typed.len()) > 3 {
                continue;
            }
            let lp = Self::context_log_p(dict, id, prev);
            let bar = second.max(rival);
            let room = if bar.is_finite() {
                ((lp - bar) / weight).min(limit)
            } else {
                limit
            };
            if room <= 0.0 {
                continue;
            }
            let Some(mut cost) = distance::weighted(&self.adj, typed, word, room) else {
                continue;
            };
            if drops_inflection(typed, word) {
                if typed.len() >= NO_DROPPED_INFLECTION_FROM {
                    continue;
                }
                cost += DROPPED_INFLECTION;
            }
            let score = lp - weight * cost;
            match best {
                Some((_, b)) if score <= b => second = second.max(score),
                _ => {
                    if let Some((_, b)) = best {
                        second = second.max(b);
                    }
                    best = Some((id, score));
                }
            }
        }
        let (id, score) = best?;
        let margin = score - second.max(rival);
        Some(Choice { id, margin })
    }
}

#[cfg(test)]
mod tests {
    use super::drops_inflection;

    #[test]
    fn dropped_inflections() {
        let drops = |t: &str, c: &str| drops_inflection(t.as_bytes(), c.as_bytes());
        assert!(drops("jumsp", "jump"));
        assert!(drops("returs", "return"));
        assert!(drops("happend", "happens"));
        assert!(drops("happend", "happen"));
        assert!(!drops("jumsp", "jumps"));
        assert!(!drops("happend", "happened"));
        assert!(
            !drops("cast", "cart"),
            "an s second to last needs a shorter candidate"
        );
        assert!(!drops("classs", "class"), "a doubled s");
        assert!(!drops("its", "it"), "too short to tell");
        assert!(!drops("alredad", "already"), "a vowel before the d");
    }
}
