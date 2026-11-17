//! Every external address the app opens or calls. The settings window has its own copy of the
//! links in `app/src/links.ts`; a test keeps the two in step.

/// GitHub Sponsors page.
pub const SPONSOR_URL: &str = "https://github.com/sponsors/ReimagineMedias";
/// The source repository, for "Star on GitHub".
pub const REPO_URL: &str = "https://github.com/ReimagineMedias/unflub";

/// Product-scoped API (`D:\Code\rmgn-api\docs\unflub-contract.md`).
pub const API_BASE: &str = "https://api.rmgn.ca/unflub/v1";

/// Prefixes the app may open in the browser. Mirrors the `opener:allow-open-url` scope in
/// `capabilities/default.json`, so a link from remote config can't open anything else.
pub const OPENABLE: &[&str] = &["https://github.com/", "https://unflub.rmgn.ca/"];

/// True when `url` may be opened in the browser.
pub fn openable(url: &str) -> bool {
    OPENABLE.iter().any(|p| url.starts_with(p)) && !url.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(rel: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn only_our_sites_open() {
        assert!(openable(SPONSOR_URL));
        assert!(openable(REPO_URL));
        assert!(openable("https://unflub.rmgn.ca/privacy"));
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.example/",
            "https://unflub.rmgn.ca.evil/",
            "file:///C:/Windows",
            "https://github.com/x y",
        ] {
            assert!(!openable(bad), "{bad}");
        }
    }

    /// The window's links.ts uses the same addresses.
    #[test]
    fn links_ts_matches() {
        let ts = read("../src/links.ts");
        for url in [SPONSOR_URL, REPO_URL] {
            assert!(ts.contains(&format!("\"{url}\"")), "links.ts lacks {url}");
        }
    }

    /// The capability scope allows exactly the prefixes above.
    #[test]
    fn capability_scope_matches() {
        let cap: serde_json::Value =
            serde_json::from_str(&read("capabilities/default.json")).unwrap();
        let open = cap["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["identifier"] == "opener:allow-open-url")
            .expect("opener:allow-open-url permission");
        let urls: Vec<&str> = open["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["url"].as_str().unwrap())
            .collect();
        let want: Vec<String> = OPENABLE.iter().map(|p| format!("{p}*")).collect();
        assert_eq!(urls, want);
    }

    /// External links in the window go through `ExternalLink` (the opener plugin). A raw
    /// `<a href>` or `target="_blank"` does nothing in the Tauri web view.
    #[test]
    fn window_has_no_raw_external_links() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src");
        let mut stack = vec![dir];
        let mut checked = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if !matches!(ext, "tsx" | "ts") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                checked += 1;
                for bad in ["<a ", "<a\n", "target=\"_blank\"", "window.open("] {
                    assert!(
                        !text.contains(bad),
                        "{} uses {bad:?}; use <ExternalLink> from components.tsx",
                        path.display()
                    );
                }
            }
        }
        assert!(checked > 3, "found the settings UI sources");
    }
}
