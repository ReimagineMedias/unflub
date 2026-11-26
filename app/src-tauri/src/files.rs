//! Replacing the app's small data files safely, and moving them where they belong.

use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

/// Makes every temp file name unique within this process.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Replace `path` with `bytes` so that after a crash or a power loss it holds either the old
/// content or the new, never a short or empty file: write a temp file beside it, flush it to
/// disk, then rename it over `path`. The temp file is removed if anything fails. Each call has
/// its own temp file, so two writers never write into the same one.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".{}-{seq}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        // Without this the rename can reach the disk before the data does.
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Files and folders that describe this machine, not the user's choices: the local Stats page
/// (typed words), undo counts (typed words), the usage tally, the install facts (install day,
/// tour outcome), the rollout bucket, the last update check and the cached manifest. They live in the local app data folder, so a roaming profile
/// never copies typed words to a server and one PC's update check never postpones another's.
pub const MACHINE_STATE: &[&str] = &[
    "stats.json",
    "undo-counts.json",
    "usage.json",
    "install-facts.json",
    "rollout-bucket",
    "last-update-check",
    "manifest",
];

/// The entries of [`MACHINE_STATE`] that hold typed words.
const TYPED_WORDS: &[&str] = &["stats.json", "undo-counts.json"];

/// Move each of `names` from `from` (the roaming folder, where versions before 0.2 kept them) to
/// `to`, once. An entry already in `to` wins: the old one is left where it is, unless it holds
/// typed words ([`TYPED_WORDS`], written by an older version after a downgrade), which never stay
/// in the roaming folder and are deleted. When a rename
/// can't cross volumes (a redirected AppData), it is copied and then removed; a copy that fails is
/// undone and the old entry kept.
pub fn move_machine_state(from: &Path, to: &Path, names: &[&str]) {
    if from == to {
        return;
    }
    for name in names {
        let (src, dst) = (from.join(name), to.join(name));
        if !src.exists() {
            continue;
        }
        if dst.exists() {
            if TYPED_WORDS.contains(name) {
                if let Err(e) = fs::remove_file(&src) {
                    eprintln!("data: can't remove the old {name}: {e}");
                }
                continue;
            }
            eprintln!(
                "data: {} is already in {}; the old copy stays in {}",
                name,
                to.display(),
                from.display()
            );
            continue;
        }
        let moved = fs::create_dir_all(to).and_then(|()| {
            fs::rename(&src, &dst).or_else(|_| {
                copy_all(&src, &dst)
                    .inspect_err(|_| {
                        let _ = fs::remove_dir_all(&dst);
                        let _ = fs::remove_file(&dst);
                    })
                    .and_then(|()| {
                        if src.is_dir() {
                            fs::remove_dir_all(&src)
                        } else {
                            fs::remove_file(&src)
                        }
                    })
            })
        });
        if let Err(e) = moved {
            eprintln!("data: can't move {name} to {}: {e}", to.display());
        }
    }
}

/// Copy a file, or a folder and everything in it.
fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review #120: typed words and per-machine update state move out of the roaming profile,
    /// keeping their content; an entry already in the local folder isn't replaced.
    #[test]
    fn machine_state_moves_to_the_local_folder_once() {
        let base = std::env::temp_dir().join(format!("unflub-move-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (roaming, local) = (base.join("Roaming"), base.join("Local"));
        fs::create_dir_all(roaming.join("manifest")).unwrap();
        fs::write(roaming.join("stats.json"), "typed words").unwrap();
        fs::write(roaming.join("manifest").join("state.json"), "etag").unwrap();
        fs::write(roaming.join("usage.json"), "old").unwrap();
        fs::write(roaming.join("settings.json"), "roams").unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("usage.json"), "new").unwrap();
        fs::write(roaming.join("undo-counts.json"), "old words").unwrap();
        fs::write(local.join("undo-counts.json"), "words").unwrap();

        move_machine_state(&roaming, &local, MACHINE_STATE);
        assert_eq!(
            fs::read_to_string(local.join("stats.json")).unwrap(),
            "typed words"
        );
        assert!(!roaming.join("stats.json").exists());
        assert_eq!(
            fs::read_to_string(local.join("manifest").join("state.json")).unwrap(),
            "etag"
        );
        assert!(!roaming.join("manifest").exists());
        assert_eq!(fs::read_to_string(local.join("usage.json")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(roaming.join("usage.json")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(local.join("undo-counts.json")).unwrap(),
            "words"
        );
        assert!(
            !roaming.join("undo-counts.json").exists(),
            "typed words never stay in the roaming folder"
        );
        assert!(roaming.join("settings.json").exists(), "settings roam");
        assert!(!local.join("settings.json").exists());

        // Running again changes nothing, and the same folder is left alone.
        move_machine_state(&roaming, &local, MACHINE_STATE);
        move_machine_state(&local, &local, MACHINE_STATE);
        assert_eq!(
            fs::read_to_string(local.join("stats.json")).unwrap(),
            "typed words"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn copies_folders() {
        let base = std::env::temp_dir().join(format!("unflub-copy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("a").join("b")).unwrap();
        fs::write(base.join("a").join("b").join("c"), "x").unwrap();
        copy_all(&base.join("a"), &base.join("z")).unwrap();
        assert_eq!(
            fs::read_to_string(base.join("z").join("b").join("c")).unwrap(),
            "x"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn replaces_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("unflub-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("state.json");
        replace(&path, b"one").unwrap();
        replace(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review #142: every data file goes through [`replace`], which flushes before the rename.
    /// A hand-rolled write-then-rename would skip the flush.
    #[test]
    fn no_module_renames_a_temp_file_itself() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in fs::read_dir(src).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == "files.rs") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for pattern in ["with_extension(\"tmp\")", "with_extension(\"json.tmp\")"] {
                assert!(
                    !text.contains(pattern),
                    "{} writes its own temp file; use files::replace",
                    path.display()
                );
            }
        }
    }
}
