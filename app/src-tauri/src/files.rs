//! Replacing the app's small data files safely.

use std::{
    fs,
    io::{self, Write},
    path::Path,
};

/// Replace `path` with `bytes` so that after a crash or a power loss it holds either the old
/// content or the new, never a short or empty file: write a temp file beside it, flush it to
/// disk, then rename it over `path`. The temp file is removed if anything fails.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
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

#[cfg(test)]
mod tests {
    use super::*;

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
