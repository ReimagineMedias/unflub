//! The text the engine has seen before the caret since the last reset.

use std::collections::VecDeque;

const CAPACITY: usize = 128;

/// Characters that belong to a word. Apostrophes and hyphens join contractions and compounds;
/// underscores are kept so `snake_case` stays one token.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '\'' | '\u{2019}' | '-' | '_')
}

/// Boundaries that glue code-like tokens together (`src/main`, `user@host`, `https:`), or
/// make the word before them a function name (`strlen(`). The engine never decides on these
/// when skipping code.
pub fn is_glue(c: char) -> bool {
    matches!(c, '/' | '\\' | '@' | ':' | '(')
}

/// Marks a token as code, a handle or a tag (`#rustlang`: tags are often run-together words).
fn is_code_punct(c: char) -> bool {
    matches!(c, '.' | '/' | '\\' | '@' | ':' | '_' | '#') || c.is_ascii_digit()
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
    /// The character just before `buf[0]` is known to be whitespace (or a word start).
    front_ws: bool,
    /// Inside an open inline backtick span. Closed by a newline: inline code can't span lines,
    /// and a stray backtick must not turn correction off for good.
    backticks: bool,
    /// Inside a ```` ``` ```` fence (opened by three backticks at the start of a line).
    fence: bool,
    /// Bumped by every reset: anything learned about the caret before it is stale.
    pub generation: u64,
}

impl Session {
    pub fn new() -> Self {
        Self {
            buf: VecDeque::with_capacity(CAPACITY),
            front_ws: false,
            backticks: false,
            fence: false,
            generation: 0,
        }
    }

    /// Inside a backtick span or a code fence.
    pub fn in_backticks(&self) -> bool {
        self.backticks || self.fence
    }

    /// The buffer ends with ```` ``` ```` that starts its line (only spaces or tabs before it).
    fn ends_with_fence_marker(&self) -> bool {
        let n = self.buf.len();
        if n < 3 || self.buf.range(n - 3..).any(|&c| c != '`') {
            return false;
        }
        let mut i = n - 3;
        while i > 0 && matches!(self.buf[i - 1], ' ' | '\t') {
            i -= 1;
        }
        i == 0 || matches!(self.buf[i - 1], '\n' | '\r')
    }

    /// Account for the backtick just pushed (or about to be popped): the third of a fence marker
    /// opens or closes the fence (the first two toggled the inline span on and off again).
    fn toggle_backtick(&mut self) {
        if self.ends_with_fence_marker() {
            self.fence = !self.fence;
        } else {
            self.backticks = !self.backticks;
        }
    }

    /// Forget everything: the caret may now be anywhere, even mid-word.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.front_ws = false;
        self.backticks = false;
        self.fence = false;
        self.generation += 1;
    }

    /// Forget everything but that the caret is at a word start (after Ctrl+Backspace).
    pub fn reset_at_word_start(&mut self) {
        self.reset();
        self.front_ws = true;
    }

    /// The adapter found that the text before the caret ended at a word boundary when this
    /// session started (empty field, or whitespace before the caret). Taken only while nothing
    /// but the first word has been typed: then that word counts as fully observed. Returns
    /// whether it was taken.
    pub fn starts_at_word_start(&mut self) -> bool {
        if self.front_ws || self.buf.iter().any(|&c| !is_word_char(c)) {
            return false;
        }
        self.front_ws = true;
        true
    }

    pub fn push(&mut self, c: char) {
        if self.buf.len() == CAPACITY {
            if let Some(dropped) = self.buf.pop_front() {
                self.front_ws = dropped.is_whitespace();
            }
        }
        self.buf.push_back(c);
        match c {
            '`' => self.toggle_backtick(),
            '\n' | '\r' => self.backticks = false,
            _ => {}
        }
    }

    pub fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.push(c);
        }
    }

    /// Backspace. Deleting past what we saw leaves us not knowing what precedes the caret.
    pub fn pop(&mut self) {
        match self.buf.back() {
            Some('`') => self.toggle_backtick(),
            Some(_) => {}
            None => return self.reset(),
        }
        self.buf.pop_back();
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
        // `(` right after a letter or digit is a call (`strlen(lenght`); at the token start it
        // opens an aside (`(hello`).
        let code_like = (token_start..start).any(|i| {
            let c = self.buf[i];
            is_code_punct(c) || (c == '(' && i > token_start && self.buf[i - 1].is_alphanumeric())
        });
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
    fn backtick_spans_and_fences() {
        let mut s = session("a `b");
        assert!(s.in_backticks());
        s.push('\n');
        assert!(!s.in_backticks(), "inline spans end at the line");
        s.push_str("```");
        assert!(s.in_backticks());
        s.push_str("\nx\n");
        assert!(s.in_backticks(), "fences span lines");
        s.pop_n(3);
        s.pop();
        assert!(!s.in_backticks(), "backspacing the marker closes the fence");
        assert!(!s.backticks && !s.fence);
    }

    #[test]
    fn code_tokens() {
        assert!(session("src/main").current().unwrap().code_like);
        assert!(session("x.com").current().unwrap().code_like);
        assert!(session("#rustlang").current().unwrap().code_like);
        assert!(!session("(hello").current().unwrap().code_like);
        assert!(session("strlen(lenght").current().unwrap().code_like);
    }
}
