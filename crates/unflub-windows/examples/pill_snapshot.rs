//! Render the correction pill offscreen for each style at 100% and 150% and save PNGs to
//! `.local/pill/` at the workspace root. No window is created and nothing is shown on screen.
//!
//! ```text
//! cargo run -p unflub-windows --example pill_snapshot
//! ```

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    eprintln!("pill_snapshot renders with Direct2D and runs on Windows only");
}

#[cfg(windows)]
mod win {
    use std::path::PathBuf;

    use unflub_core::{Replacement, Source};
    use unflub_windows::overlay::{render_preview, PillPosition};
    use unflub_windows::{Correction, PillConfig, PillStyle};

    pub fn run() {
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/pill");
        std::fs::create_dir_all(&out).expect("create .local/pill");
        let fix = Correction::fixed(&Replacement {
            delete: 7,
            insert: "receive ".into(),
            original: "recieve".into(),
            corrected: "receive".into(),
            source: Source::Bundled,
        });
        let undo = Correction::undone(&Replacement {
            delete: 8,
            insert: "recieve".into(),
            original: "recieve".into(),
            corrected: "receive".into(),
            source: Source::Bundled,
        });
        let mut shots: Vec<(String, PillConfig, &Correction, u32)> = Vec::new();
        for style in [
            PillStyle::UnderlineCapsule,
            PillStyle::Capsule,
            PillStyle::Underline,
        ] {
            for dpi in [96, 144] {
                let config = PillConfig {
                    style,
                    ..PillConfig::default()
                };
                shots.push((format!("{style}@{}", dpi * 100 / 96), config, &fix, dpi));
            }
        }
        shots.push((
            "underline-capsule-below@100".into(),
            PillConfig {
                position: PillPosition::Below,
                ..PillConfig::default()
            },
            &fix,
            96,
        ));
        shots.push((
            "capsule-undone@150".into(),
            PillConfig {
                style: PillStyle::Capsule,
                ..PillConfig::default()
            },
            &undo,
            144,
        ));
        for (name, config, correction, dpi) in shots {
            let p = render_preview(&config, correction, dpi, "We will ")
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let path = out.join(format!("{name}.png"));
            std::fs::write(&path, png(p.width, p.height, &p.bgra)).expect("write png");
            println!("{}", path.canonicalize().unwrap_or(path).display());
        }
    }

    /// Minimal RGB PNG encoder (stored deflate blocks). Input is opaque BGRA.
    fn png(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
        let mut raw = Vec::with_capacity((width * height * 3 + height) as usize);
        for row in bgra.chunks_exact(width as usize * 4) {
            raw.push(0); // filter: none
            for px in row.chunks_exact(4) {
                raw.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        let mut zlib = vec![0x78, 0x01];
        let mut blocks = raw.chunks(65_535).peekable();
        while let Some(block) = blocks.next() {
            zlib.push(u8::from(blocks.peek().is_none()));
            let len = block.len() as u16;
            zlib.extend_from_slice(&len.to_le_bytes());
            zlib.extend_from_slice(&(!len).to_le_bytes());
            zlib.extend_from_slice(block);
        }
        zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &zlib);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + u32::from(x)) % 65_521;
            b = (b + a) % 65_521;
        }
        b << 16 | a
    }
}
