//! Windows version resource for `unflub-engine.exe`: the same company, product and copyright as
//! `unflub.exe` (whose resource Tauri writes from `app/src-tauri/tauri.conf.json`), so both show
//! the same publisher in Task Manager, file properties and installer scans. `tests/ipc.rs`
//! checks the strings against tauri.conf.json.

fn main() {
    #[cfg(windows)]
    resource::compile();
}

#[cfg(windows)]
mod resource {
    use std::{env, fs, path::PathBuf};

    const COMPANY: &str = "Reimagine Media (RMGN)";
    const COPYRIGHT: &str = "\u{a9} 2026 Matias Suxo";

    pub fn compile() {
        let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let icon = manifest.join("../../app/src-tauri/icons/icon.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let version = env::var("CARGO_PKG_VERSION").unwrap();
        let mut parts: Vec<u16> = version
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        parts.resize(4, 0);
        let numeric = parts
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let icon = icon.display().to_string().replace('\\', "\\\\");
        let rc = format!(
            r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "{COMPANY}"
      VALUE "FileDescription", "unflub keyboard engine"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "unflub-engine"
      VALUE "LegalCopyright", "{COPYRIGHT}"
      VALUE "OriginalFilename", "unflub-engine.exe"
      VALUE "ProductName", "unflub"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
        );
        let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("unflub-engine.rc");
        fs::write(&out, rc).unwrap();
        embed_resource::compile(&out, embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
