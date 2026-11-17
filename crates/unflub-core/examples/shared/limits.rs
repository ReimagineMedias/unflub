//! The small subset of TOML that `perf/limits.toml` uses: `[section]` headers, `key = number`
//! lines and `#` comments. Shared by the `bench` and `quality` examples (`#[path]`), so neither
//! needs dependencies.

pub struct Check {
    pub metric: &'static str,
    pub value: Option<f64>,
    pub limit: f64,
    /// `None` when the metric isn't available on this OS.
    pub pass: Option<bool>,
}

pub fn load(path: &str, section: &str) -> Result<Vec<(String, f64)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse(&text, section)
}

pub fn parse(text: &str, section: &str) -> Result<Vec<(String, f64)>, String> {
    let mut current = String::new();
    let mut found = false;
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let l = raw.split('#').next().unwrap_or_default().trim();
        if l.is_empty() {
            continue;
        }
        if let Some(name) = l.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim().to_owned();
            found |= current == section;
            continue;
        }
        let (key, value) = l
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected `key = number`", i + 1))?;
        let value: f64 = value
            .trim()
            .replace('_', "")
            .parse()
            .map_err(|_| format!("line {}: {:?} is not a number", i + 1, value.trim()))?;
        if current == section {
            out.push((key.trim().to_owned(), value));
        }
    }
    if !found {
        return Err(format!("no [{section}] section"));
    }
    Ok(out)
}

/// One check per limit in the section: the value may not exceed it. A limit naming no metric is
/// an error, so a typo can't silently disable a check.
#[allow(dead_code)]
pub fn compare(
    limits: &[(String, f64)],
    metrics: &[(&'static str, Option<f64>)],
) -> Result<Vec<Check>, String> {
    compare_with(limits, metrics, &[])
}

/// Like [`compare`], but the metrics named in `at_least` are minimums (higher is better, such as
/// a recovery rate): the value may not fall below the limit.
pub fn compare_with(
    limits: &[(String, f64)],
    metrics: &[(&'static str, Option<f64>)],
    at_least: &[&str],
) -> Result<Vec<Check>, String> {
    limits
        .iter()
        .map(|(key, limit)| {
            let (metric, value) = metrics
                .iter()
                .find(|(m, _)| m == key)
                .ok_or_else(|| format!("unknown metric {key:?}"))?;
            Ok(Check {
                metric,
                value: *value,
                limit: *limit,
                pass: value.map(|v| {
                    if at_least.contains(metric) {
                        v >= *limit
                    } else {
                        v <= *limit
                    }
                }),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# limits\n[core]\nstartup_ms = 600 # measured 100\n\
                        heap_bytes = 25_165_824\n\n[lowend.core]\nstartup_ms = 2000\n";

    #[test]
    fn parses_one_section() {
        let core = parse(TEXT, "core").unwrap();
        assert_eq!(
            core,
            vec![
                ("startup_ms".to_owned(), 600.0),
                ("heap_bytes".to_owned(), 25_165_824.0)
            ]
        );
        let low = parse(TEXT, "lowend.core").unwrap();
        assert_eq!(low, vec![("startup_ms".to_owned(), 2000.0)]);
        assert!(parse(TEXT, "idle").is_err());
        assert!(parse("[core]\nx = fast\n", "core").is_err());
        assert!(parse("[core]\nx\n", "core").is_err());
    }

    #[test]
    fn compares_against_limits() {
        let limits = parse(TEXT, "core").unwrap();
        let checks = compare(
            &limits,
            &[("startup_ms", Some(700.0)), ("heap_bytes", None)],
        )
        .unwrap();
        assert_eq!(checks[0].pass, Some(false));
        assert_eq!(checks[1].pass, None, "unavailable metrics are skipped");
        assert!(compare(&limits, &[("startup_ms", Some(1.0))]).is_err());
    }

    #[test]
    fn minimums_fail_below_the_limit() {
        let limits = parse(
            "[q]
exact_pct = 84
wrong_pct = 2.5
",
            "q",
        )
        .unwrap();
        let check = |exact, wrong| {
            compare_with(
                &limits,
                &[("exact_pct", Some(exact)), ("wrong_pct", Some(wrong))],
                &["exact_pct"],
            )
            .unwrap()
            .iter()
            .map(|c| c.pass)
            .collect::<Vec<_>>()
        };
        assert_eq!(check(85.0, 2.0), [Some(true), Some(true)]);
        assert_eq!(check(83.9, 2.0), [Some(false), Some(true)]);
        assert_eq!(check(90.0, 2.6), [Some(true), Some(false)]);
    }
}
