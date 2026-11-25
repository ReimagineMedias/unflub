//! The coarse kind of app a fix happened in, for the anonymous usage stats ("fixes in
//! browsers, chat, code editors ..."). Decided here, in the engine, from the focused app's
//! executable name; only the category crosses the pipe. The name itself is never sent to the
//! app, stored or reported.
//!
//! Terminals never get fixes, so they have no category.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    Browser,
    Chat,
    CodeEditor,
    Office,
    Notes,
    Email,
    #[default]
    Other,
}

/// Executable names (lowercased, without `.exe`) and their category. Anything else is
/// [`AppCategory::Other`].
const APPS: &[(&str, AppCategory)] = {
    use AppCategory::*;
    &[
        // Browsers.
        ("chrome", Browser),
        ("msedge", Browser),
        ("firefox", Browser),
        ("brave", Browser),
        ("opera", Browser),
        ("vivaldi", Browser),
        ("arc", Browser),
        ("zen", Browser),
        ("librewolf", Browser),
        ("waterfox", Browser),
        ("floorp", Browser),
        ("chromium", Browser),
        ("thorium", Browser),
        ("iexplore", Browser),
        ("duckduckgo", Browser),
        ("comet", Browser),
        // Chat.
        ("discord", Chat),
        ("slack", Chat),
        ("teams", Chat),
        ("ms-teams", Chat),
        ("whatsapp", Chat),
        ("whatsapp.root", Chat),
        ("telegram", Chat),
        ("signal", Chat),
        ("element", Chat),
        ("zoom", Chat),
        ("skype", Chat),
        ("messenger", Chat),
        ("wechat", Chat),
        ("weixin", Chat),
        ("line", Chat),
        ("viber", Chat),
        ("mattermost", Chat),
        ("beeper", Chat),
        ("rocket.chat", Chat),
        // Code editors and IDEs.
        ("code", CodeEditor),
        ("code - insiders", CodeEditor),
        ("vscodium", CodeEditor),
        ("cursor", CodeEditor),
        ("windsurf", CodeEditor),
        ("zed", CodeEditor),
        ("idea64", CodeEditor),
        ("pycharm64", CodeEditor),
        ("webstorm64", CodeEditor),
        ("clion64", CodeEditor),
        ("rider64", CodeEditor),
        ("goland64", CodeEditor),
        ("phpstorm64", CodeEditor),
        ("rubymine64", CodeEditor),
        ("datagrip64", CodeEditor),
        ("rustrover64", CodeEditor),
        ("studio64", CodeEditor),
        ("devenv", CodeEditor),
        ("sublime_text", CodeEditor),
        ("notepad++", CodeEditor),
        ("t3code", CodeEditor),
        ("fleet", CodeEditor),
        ("kiro", CodeEditor),
        ("trae", CodeEditor),
        // Office.
        ("winword", Office),
        ("excel", Office),
        ("powerpnt", Office),
        ("onenote", Office),
        ("msaccess", Office),
        ("mspub", Office),
        ("visio", Office),
        ("soffice", Office),
        ("soffice.bin", Office),
        ("wps", Office),
        ("et", Office),
        ("wpp", Office),
        // Notes.
        ("obsidian", Notes),
        ("notion", Notes),
        ("notepad", Notes),
        ("logseq", Notes),
        ("evernote", Notes),
        ("joplin", Notes),
        ("typora", Notes),
        ("anytype", Notes),
        ("upnote", Notes),
        ("standard notes", Notes),
        ("simplenote", Notes),
        ("wordpad", Notes),
        ("stickynotes", Notes),
        // Email.
        ("outlook", Email),
        ("olk", Email),
        ("thunderbird", Email),
        ("betterbird", Email),
        ("mailspring", Email),
        ("hxoutlook", Email),
        ("mailclient", Email),
    ]
};

/// The category of the focused app, from its executable name (any case, with or without
/// `.exe`). Unknown or missing: [`AppCategory::Other`].
pub fn category_of(exe: Option<&str>) -> AppCategory {
    let Some(exe) = exe else {
        return AppCategory::Other;
    };
    let lower = exe.to_ascii_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    APPS.iter()
        .find(|(n, _)| *n == name)
        .map_or(AppCategory::Other, |(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_apps_get_their_category() {
        for (exe, want) in [
            ("chrome.exe", AppCategory::Browser),
            ("MSEDGE.EXE", AppCategory::Browser),
            ("firefox.exe", AppCategory::Browser),
            ("discord.exe", AppCategory::Chat),
            ("ms-teams.exe", AppCategory::Chat),
            ("slack.exe", AppCategory::Chat),
            ("code.exe", AppCategory::CodeEditor),
            ("Code - Insiders.exe", AppCategory::CodeEditor),
            ("cursor.exe", AppCategory::CodeEditor),
            ("notepad++.exe", AppCategory::CodeEditor),
            ("t3code.exe", AppCategory::CodeEditor),
            ("winword.exe", AppCategory::Office),
            ("onenote.exe", AppCategory::Office),
            ("obsidian.exe", AppCategory::Notes),
            ("notepad.exe", AppCategory::Notes),
            ("olk.exe", AppCategory::Email),
            ("thunderbird", AppCategory::Email),
        ] {
            assert_eq!(category_of(Some(exe)), want, "{exe}");
        }
    }

    #[test]
    fn everything_else_is_other() {
        for exe in [
            None,
            Some(""),
            Some("explorer.exe"),
            Some("msedgewebview2.exe"),
            Some("chrome.exe.bak"),
            Some("mychrome.exe"),
        ] {
            assert_eq!(category_of(exe), AppCategory::Other, "{exe:?}");
        }
    }

    #[test]
    fn names_are_lowercase_and_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in APPS {
            assert_eq!(*name, name.to_lowercase());
            assert!(!name.ends_with(".exe"), "{name}");
            assert!(seen.insert(*name), "{name} twice");
        }
    }

    #[test]
    fn the_wire_names_match_the_report() {
        let names: Vec<String> = [
            AppCategory::Browser,
            AppCategory::Chat,
            AppCategory::CodeEditor,
            AppCategory::Office,
            AppCategory::Notes,
            AppCategory::Email,
            AppCategory::Other,
        ]
        .iter()
        .map(|c| serde_json::to_string(c).unwrap())
        .collect();
        assert_eq!(
            names,
            [
                "\"browser\"",
                "\"chat\"",
                "\"code_editor\"",
                "\"office\"",
                "\"notes\"",
                "\"email\"",
                "\"other\""
            ]
        );
    }
}
