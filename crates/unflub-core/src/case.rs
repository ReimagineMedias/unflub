//! Letter-case patterns, so a fix keeps the shape of what was typed.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Lower,
    /// First letter uppercase, the rest lowercase (also a lone capital such as `I`).
    Title,
    /// Two or more letters, all uppercase.
    Upper,
    Mixed,
}

pub fn pattern(word: &str) -> Case {
    let mut letters = word.chars().filter(|c| c.is_alphabetic());
    let Some(first) = letters.next() else {
        return Case::Lower;
    };
    let (mut upper, mut lower, mut count) = (0, 0, 0);
    for c in letters {
        count += 1;
        if c.is_uppercase() {
            upper += 1;
        } else if c.is_lowercase() {
            lower += 1;
        }
    }
    match (first.is_uppercase(), upper, lower) {
        (false, 0, _) => Case::Lower,
        (true, 0, _) => Case::Title,
        (true, u, 0) if u == count => Case::Upper,
        _ => Case::Mixed,
    }
}

/// Applies `case` to a lowercase replacement.
pub fn apply(case: Case, word: &str) -> String {
    match case {
        Case::Upper => word.to_uppercase(),
        Case::Title => {
            let mut chars = word.chars();
            match chars.next() {
                Some(f) => f.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
        Case::Lower | Case::Mixed => word.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert_eq!(pattern("teh"), Case::Lower);
        assert_eq!(pattern("Teh"), Case::Title);
        assert_eq!(pattern("I'm"), Case::Title);
        assert_eq!(pattern("I"), Case::Title);
        assert_eq!(pattern("TEH"), Case::Upper);
        assert_eq!(pattern("useState"), Case::Mixed);
        assert_eq!(pattern("McDonald"), Case::Mixed);
        assert_eq!(pattern("tEH"), Case::Mixed);
    }

    #[test]
    fn apply_case() {
        assert_eq!(apply(Case::Title, "the"), "The");
        assert_eq!(apply(Case::Upper, "the"), "THE");
        assert_eq!(apply(Case::Lower, "the"), "the");
    }
}
