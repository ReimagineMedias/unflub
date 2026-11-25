//! Every external address the app opens or calls. The settings window has its own copy of the
//! links in `app/src/links.ts`; a test keeps the two in step.

/// GitHub Sponsors page.
pub const SPONSOR_URL: &str = "https://github.com/sponsors/ReimagineMedias";
/// The source repository, for "Star on GitHub".
pub const REPO_URL: &str = "https://github.com/ReimagineMedias/unflub";

/// Product-scoped API, for usage stats only (`D:\Code\rmgn-api\docs\unflub-contract.md`).
pub const API_BASE: &str = "https://api.rmgn.net/unflub/v1";

/// Static files on the CDN: the signed manifest (`manifest.rs`), each release's Tauri file and
/// the installers.
pub const DL_BASE: &str = "https://dl.rmgn.net/unflub";

/// What the app may open in the browser: an entry ending in `/` is a prefix, anything else must
/// match exactly. Only our own GitHub owner, so a message link can't point at someone else's
/// repository or release. Mirrors the `opener:allow-open-url` scope in
/// `capabilities/default.json` and the API's `messageLinks` (rmgn-api `src/products/unflub.ts`).
pub const OPENABLE: &[&str] = &[
    "https://github.com/ReimagineMedias/",
    SPONSOR_URL,
    "https://unflub.rmgn.net/",
];

/// Where installers are published: the GitHub Release assets.
pub const RELEASE_DOWNLOADS: &str = "https://github.com/ReimagineMedias/unflub/releases/download/";

/// An installer URL the signed manifest may name: a GitHub Release asset, or a dl.rmgn.net
/// address that redirects to one.
pub fn installer_url_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && (url.as_str().starts_with(RELEASE_DOWNLOADS)
            || (url.host_str() == Some("dl.rmgn.net") && url.path().starts_with("/unflub/")))
}

/// Where an installer download may be redirected: the GitHub Release asset and the GitHub
/// hosts that serve its bytes. The installer's signature is what proves the bytes; this keeps
/// the download from wandering anywhere else.
pub fn redirect_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && (url.as_str().starts_with(RELEASE_DOWNLOADS)
            || matches!(
                url.host_str(),
                Some("objects.githubusercontent.com" | "release-assets.githubusercontent.com")
            ))
}

/// True when `url` may be opened in the browser.
pub fn openable(url: &str) -> bool {
    let allowed = OPENABLE.iter().any(|p| {
        if p.ends_with('/') {
            url.starts_with(p)
        } else {
            url == *p
        }
    });
    // No "/../" either: the browser would resolve it out of the allowed prefix.
    allowed
        && !url.contains(char::is_whitespace)
        && !url.contains("/..")
        && !url.contains("%2e")
        && !url.contains("%2E")
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
        assert!(openable("https://unflub.rmgn.net/privacy"));
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.example/",
            "https://unflub.rmgn.net.evil/",
            "https://unflub.rmgn.ca/",
            "file:///C:/Windows",
            "https://github.com/x y",
            "https://github.com/someone-else/unflub/releases",
            "https://github.com/ReimagineMediasX/",
            "https://github.com/ReimagineMedias/../evil/x",
            "https://github.com/ReimagineMedias/%2e%2e/evil",
            "https://github.com/sponsors/ReimagineMediasX",
        ] {
            assert!(!openable(bad), "{bad}");
        }
    }

    /// Everything unflub calls is https on rmgn.net, plus GitHub for the installers.
    #[test]
    fn network_bases() {
        assert_eq!(API_BASE, "https://api.rmgn.net/unflub/v1");
        assert_eq!(DL_BASE, "https://dl.rmgn.net/unflub");
    }

    #[test]
    fn installers_come_only_from_github_releases_or_dl() {
        let u = |s: &str| reqwest::Url::parse(s).unwrap();
        for ok in [
            "https://github.com/ReimagineMedias/unflub/releases/download/v0.2.0/unflub_0.2.0_x64-setup.exe",
            "https://dl.rmgn.net/unflub/0.2.0/unflub_0.2.0_x64-setup.exe",
        ] {
            assert!(installer_url_allowed(&u(ok)), "{ok}");
        }
        for bad in [
            "http://github.com/ReimagineMedias/unflub/releases/download/v0.2.0/x.exe",
            "https://github.com/someone/unflub/releases/download/v0.2.0/x.exe",
            "https://github.com/ReimagineMedias/unflub/archive/x.zip",
            "https://dl.rmgn.net/other/x.exe",
            "https://dl.rmgn.net.evil/unflub/x.exe",
            "https://objects.githubusercontent.com/x",
        ] {
            assert!(!installer_url_allowed(&u(bad)), "{bad}");
        }
        for ok in [
            "https://github.com/ReimagineMedias/unflub/releases/download/v0.2.0/x.exe",
            "https://objects.githubusercontent.com/github-production-release-asset/1/2",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/2",
        ] {
            assert!(redirect_allowed(&u(ok)), "{ok}");
        }
        for bad in [
            "http://objects.githubusercontent.com/x",
            "https://evil.example/x.exe",
            "https://github.com/someone/else/releases/download/v1/x.exe",
            "https://raw.githubusercontent.com/ReimagineMedias/unflub/main/x",
            "https://dl.rmgn.net/unflub/0.2.0/x.exe",
        ] {
            assert!(!redirect_allowed(&u(bad)), "{bad}");
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
        // Prefixes get a trailing wildcard; exact entries stay exact.
        let want: Vec<String> = OPENABLE
            .iter()
            .map(|p| {
                if p.ends_with('/') {
                    format!("{p}*")
                } else {
                    (*p).to_string()
                }
            })
            .collect();
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
