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

/// Words written with a capital wherever they appear (months, weekdays, holidays, languages and
/// nationalities, places, brands), sorted. The bundled dictionary is all lowercase, so a fix
/// landing on one of these is capitalised. Words that are also common lowercase words (`may`,
/// `march`, `august`, `china`, `turkey`, `polish`, `google`, `apple`, `windows`) are left out.
const ALWAYS_CAPITALISED: &[&str] = &[
    "africa",
    "african",
    "alberta",
    "america",
    "american",
    "april",
    "arabic",
    "asia",
    "asian",
    "australia",
    "australian",
    "berlin",
    "boston",
    "brazil",
    "brazilian",
    "britain",
    "british",
    "calgary",
    "california",
    "canada",
    "canadian",
    "chicago",
    "chinese",
    "christmas",
    "danish",
    "december",
    "dutch",
    "easter",
    "edmonton",
    "egypt",
    "england",
    "english",
    "europe",
    "european",
    "facebook",
    "february",
    "finnish",
    "florida",
    "france",
    "french",
    "friday",
    "german",
    "germany",
    "greek",
    "halloween",
    "hanukkah",
    "hebrew",
    "hindi",
    "india",
    "indian",
    "instagram",
    "ireland",
    "irish",
    "israel",
    "italian",
    "italy",
    "january",
    "japan",
    "japanese",
    "july",
    "june",
    "kenya",
    "korea",
    "korean",
    "latin",
    "linkedin",
    "london",
    "mexican",
    "mexico",
    "microsoft",
    "monday",
    "montreal",
    "netflix",
    "nigeria",
    "norwegian",
    "november",
    "october",
    "ontario",
    "ottawa",
    "pakistan",
    "paris",
    "portugal",
    "portuguese",
    "quebec",
    "ramadan",
    "russia",
    "russian",
    "saturday",
    "scotland",
    "scottish",
    "seattle",
    "september",
    "spain",
    "spanish",
    "spotify",
    "sunday",
    "swedish",
    "texas",
    "thanksgiving",
    "thursday",
    "tokyo",
    "toronto",
    "tuesday",
    "ukraine",
    "ukrainian",
    "vancouver",
    "vietnam",
    "wednesday",
    "winnipeg",
    "youtube",
];

/// A lowercase dictionary word that is always written with a capital (`canada`, `wednesday`).
pub fn always_capitalised(word: &str) -> bool {
    ALWAYS_CAPITALISED.binary_search(&word).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_capitalised_list() {
        assert!(ALWAYS_CAPITALISED.windows(2).all(|w| w[0] < w[1]), "sorted");
        assert!(ALWAYS_CAPITALISED
            .iter()
            .all(|w| w.bytes().all(|b| b.is_ascii_lowercase())));
        assert!(always_capitalised("canada") && always_capitalised("wednesday"));
        assert!(!always_capitalised("may") && !always_capitalised("the"));
    }

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
