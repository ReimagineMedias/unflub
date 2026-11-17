//! The text the engine has seen before the caret since the last reset.

use std::collections::VecDeque;

const CAPACITY: usize = 128;

/// Characters that belong to a word. Apostrophes and hyphens join contractions and compounds;
/// underscores are kept so `snake_case` stays one token.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '\'' | '\u{2019}' | '-' | '_')
}

/// Boundaries that glue code-like tokens together (`src/main`, `user@host`, `https:`).
/// The engine never decides on these when skipping code.
pub fn is_glue(c: char) -> bool {
    matches!(c, '/' | '\\' | '@' | ':')
}

fn is_code_punct(c: char) -> bool {
    matches!(c, '.' | '/' | '\\' | '@' | ':' | '_') || c.is_ascii_digit()
}

/// The word just before the caret, located in the session buffer.
pub struct Current {
    pub start: usize,
    pub end: usize,
    /// The whitespace-delimited token around the word holds code punctuation or digits.
    pub code_like: bool,
    /// Range of the previous word if it is separated from this one by spaces only.
    pub prev: Option<(usize, usize)>,
    /// Nothing observed before the word, or it follows a newline or `.`, `!`, `?`.
    pub sentence_start: bool,
}

pub struct Session {
    buf: VecDeque<char>,
    /// The character just before `buf[0]` is known to be whitespace.
    front_ws: bool,
    /// Inside an open backtick span.
    pub backticks: bool,
}

impl Session {
    pub fn new() -> Self {
        Self {
            buf: VecDeque::with_capacity(CAPACITY),
            front_ws: false,
            backticks: false,
        }
    }

    /// Forget everything: the caret may now be anywhere, even mid-word.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.front_ws = false;
        self.backticks = false;
    }

    pub fn push(&mut self, c: char) {
        if self.buf.len() == CAPACITY {
            if let Some(dropped) = self.buf.pop_front() {
                self.front_ws = dropped.is_whitespace();
            }
        }
        if c == '`' {
            self.backticks = !self.backticks;
        }
        self.buf.push_back(c);
    }

    pub fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.push(c);
        }
    }

    /// Backspace. Deleting past what we saw leaves us not knowing what precedes the caret.
    pub fn pop(&mut self) {
        match self.buf.pop_back() {
            Some('`') => self.backticks = !self.backticks,
            Some(_) => {}
            None => self.reset(),
        }
    }

    pub fn pop_n(&mut self, n: usize) {
        if n > self.buf.len() {
            self.reset();
            return;
        }
        for _ in 0..n {
            self.pop();
        }
    }

    pub fn chars(&self, start: usize, end: usize) -> impl Iterator<Item = char> + '_ {
        self.buf.range(start..end).copied()
    }

    /// The fully observed word ending at the caret, if any.
    pub fn current(&self) -> Option<Current> {
        let end = self.buf.len();
        let mut start = end;
        while start > 0 && is_word_char(self.buf[start - 1]) {
            start -= 1;
        }
        if start == end {
            return None;
        }
        let mut token_start = start;
        while token_start > 0 && !self.buf[token_start - 1].is_whitespace() {
            token_start -= 1;
        }
        if token_start == 0 && !self.front_ws {
            return None;
        }
        let code_like = self
            .buf
            .range(token_start..start)
            .any(|&c| is_code_punct(c));
        Some(Current {
            start,
            end,
            code_like,
            prev: self.prev_word(token_start),
            sentence_start: self.sentence_start(token_start),
        })
    }

    fn sentence_start(&self, token_start: usize) -> bool {
        let mut i = token_start;
        while i > 0 && self.buf[i - 1].is_whitespace() {
            if matches!(self.buf[i - 1], '\n' | '\r') {
                return true;
            }
            i -= 1;
        }
        i == 0 || matches!(self.buf[i - 1], '.' | '!' | '?')
    }

    fn prev_word(&self, token_start: usize) -> Option<(usize, usize)> {
        let mut end = token_start;
        while end > 0 && matches!(self.buf[end - 1], ' ' | '\t') {
            end -= 1;
        }
        if end == token_start || end == 0 || self.buf[end - 1].is_whitespace() {
            return None;
        }
        let mut start = end;
        while start > 0 && !self.buf[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == 0 && !self.front_ws {
            return None;
        }
        self.buf
            .range(start..end)
            .all(|&c| c.is_alphabetic() || c == '\'' || c == '\u{2019}')
            .then_some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(text: &str) -> Session {
        let mut s = Session::new();
        s.front_ws = true;
        s.push_str(text);
        s
    }

    fn word(s: &Session) -> Option<String> {
        s.current().map(|c| s.chars(c.start, c.end).collect())
    }

    #[test]
    fn finds_word_and_previous() {
        let s = session("hello wrold");
        let c = s.current().unwrap();
        assert_eq!(word(&s).as_deref(), Some("wrold"));
        let (a, b) = c.prev.unwrap();
        assert_eq!(s.chars(a, b).collect::<String>(), "hello");
        assert!(!c.code_like);
    }

    #[test]
    fn sentence_end_drops_context() {
        assert!(session("done. teh").current().unwrap().prev.is_none());
        assert!(session("done\nteh").current().unwrap().prev.is_none());
        assert!(session("done, teh").current().unwrap().prev.is_none());
    }

    #[test]
    fn unknown_start_is_not_observed() {
        let mut s = Session::new();
        s.push_str("teh");
        assert!(s.current().is_none());
        s.push(' ');
        s.push_str("teh");
        assert_eq!(word(&s).as_deref(), Some("teh"));
        // Deleting past what we saw.
        let mut s = session("ab");
        s.pop_n(3);
        s.push_str("cd");
        assert!(s.current().is_none());
    }

    #[test]
    fn sentence_starts() {
        assert!(session("teh").current().unwrap().sentence_start);
        assert!(session("Done. Teh").current().unwrap().sentence_start);
        assert!(session("done\nteh").current().unwrap().sentence_start);
        assert!(!session("I teh").current().unwrap().sentence_start);
        assert!(!session("so, teh").current().unwrap().sentence_start);
    }

    #[test]
    fn code_tokens() {
        assert!(session("src/main").current().unwrap().code_like);
        assert!(session("x.com").current().unwrap().code_like);
        assert!(!session("(hello").current().unwrap().code_like);
    }
}
