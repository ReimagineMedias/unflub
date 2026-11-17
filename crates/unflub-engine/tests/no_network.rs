//! The part of unflub that sees your keys has no network code. This test enforces it.
//!
//! It asks Cargo for `unflub-engine`'s normal dependency tree on every target and fails on any
//! crate that can talk to the network (HTTP clients and servers, TLS, Sentry, Tauri), and on
//! any `Win32_Networking*` feature of the `windows` crate. CI runs it as its own step:
//! `cargo test -p unflub-engine --test no_network -- --nocapture`.

use std::process::Command;

/// Crate families that mean network code. A crate matches if its name is one of these or
/// starts with one followed by `-` or `_` (`hyper-util`, `sentry-core`, `tauri-runtime`).
const FORBIDDEN: &[&str] = &[
    "reqwest",
    "hyper",
    "h2",
    "h3",
    "ureq",
    "isahc",
    "curl",
    "surf",
    "sentry",
    "tauri",
    "tokio-rustls",
    "rustls",
    "native-tls",
    "openssl",
    "quinn",
    "attohttpc",
    "websocket",
    "tungstenite",
];

fn cargo_tree(extra: &[&str]) -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let mut args = vec![
        "tree",
        "--manifest-path",
        manifest,
        "-p",
        "unflub-engine",
        "--target",
        "all",
        "--prefix",
        "none",
        "--locked",
    ];
    args.extend_from_slice(extra);
    let out = Command::new(cargo)
        .args(&args)
        .output()
        .expect("run cargo tree");
    assert!(
        out.status.success(),
        "cargo {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn crate_name(line: &str) -> &str {
    line.split_whitespace().next().unwrap_or("")
}

fn forbidden(name: &str) -> bool {
    FORBIDDEN.iter().any(|f| {
        name == *f
            || name
                .strip_prefix(f)
                .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('_'))
    })
}

#[test]
fn the_engine_has_no_network_crates() {
    let tree = cargo_tree(&["-e", "normal"]);
    let mut crates: Vec<&str> = tree
        .lines()
        .map(crate_name)
        .filter(|n| !n.is_empty())
        .collect();
    crates.sort_unstable();
    crates.dedup();
    let bad: Vec<&str> = crates.iter().copied().filter(|n| forbidden(n)).collect();
    println!(
        "unflub-engine depends on {} crates (normal edges, every target): {}",
        crates.len(),
        crates.join(", ")
    );
    assert!(
        crates.contains(&"unflub-engine") && crates.contains(&"windows"),
        "the tree looks wrong:\n{tree}"
    );
    assert!(bad.is_empty(), "network crates in unflub-engine: {bad:?}");
    println!("no network crates: ok");
}

#[test]
fn the_engine_enables_no_networking_features() {
    let tree = cargo_tree(&["-e", "normal,features"]);
    // Lines look like `windows feature "Win32_Foundation"`, with ` (*)` on repeats.
    let mut features: Vec<(&str, &str)> = tree
        .lines()
        .filter_map(|l| {
            let (krate, rest) = l.split_once(" feature \"")?;
            Some((krate, rest.split('"').next()?))
        })
        .collect();
    features.sort_unstable();
    features.dedup();
    assert!(
        features.contains(&("windows", "Win32_Foundation")),
        "no windows features found; the check would pass vacuously:\n{tree}"
    );
    // The windows crate's networking APIs, and winapi's (pulled in by findshlibs).
    let networking = |(krate, feature): &&(&str, &str)| match *krate {
        "windows" | "windows-sys" => feature.starts_with("Win32_Networking"),
        "winapi" => [
            "winsock", "ws2", "winhttp", "wininet", "mswsock", "iphlpapi", "winnet",
        ]
        .iter()
        .any(|n| feature.contains(n)),
        _ => false,
    };
    let bad: Vec<_> = features.iter().filter(networking).collect();
    for krate in ["windows", "winapi"] {
        let list: Vec<&str> = features
            .iter()
            .filter(|(k, _)| *k == krate)
            .map(|(_, f)| *f)
            .collect();
        println!("{krate} features in unflub-engine: {}", list.join(", "));
    }
    assert!(bad.is_empty(), "networking features: {bad:?}");
    println!("no Win32_Networking features: ok");
}

#[test]
fn the_matcher_catches_families_not_lookalikes() {
    for name in [
        "reqwest",
        "hyper-util",
        "sentry-core",
        "tauri-runtime-wry",
        "rustls",
        "h2",
    ] {
        assert!(forbidden(name), "{name}");
    }
    for name in [
        "serde",
        "windows",
        "unflub-core",
        "hypertext",
        "h2o",
        "curlew",
    ] {
        assert!(!forbidden(name), "{name}");
    }
}
