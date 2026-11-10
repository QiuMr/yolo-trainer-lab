//! Injects the app version from `tauri.conf.json` into the Rust binary.
//!
//! The version has exactly one source of truth: `tauri.conf.json`. Three consumers need it and
//! none of them should be edited by hand:
//!
//! | consumer | how it gets the version |
//! | --- | --- |
//! | Rust UI / updater | `env!("YOLO_TRAINER_VERSION")` injected here |
//! | NSIS installer name / Release tag | Tauri CLI reads the same config |
//! | `scripts/release.ps1` | reads the same config to build `latest.json` |
//!
//! Deliberately **not** `env!("CARGO_PKG_VERSION")`: that reads `Cargo.toml`, which is a second
//! file that will drift the first time someone bumps only one of the two. A wrong version in the
//! binary is worse than a compile error — the updater would silently compare against a stale
//! number and report "already up to date" forever.
//!
//! Parsing is a hand-written scan, not `serde_json`: adding a build-dependency just to read one
//! string would recompile the JSON parser on every clean build for no benefit. The top-level
//! `version` is the first `"version"` key in the file; the sanity check below rejects anything
//! that is not three numeric segments, so a stray nested key cannot slip through unnoticed.

use std::{env, fs, path::PathBuf};

fn main() {
    let config = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("tauri.conf.json");
    let raw = fs::read_to_string(&config)
        .unwrap_or_else(|error| panic!("读不到 {}: {error}", config.display()));
    let version = extract_version(&raw)
        .unwrap_or_else(|| panic!("{} 里找不到顶层 version", config.display()));

    let numeric = version
        .split('.')
        .map(|part| part.parse::<u64>())
        .collect::<Result<Vec<_>, _>>();
    assert!(
        matches!(numeric, Ok(ref parts) if parts.len() == 3),
        "版本号必须是 x.y.z 数字段，实际是 {version:?}；发版脚本与签名都依赖这个格式"
    );

    println!("cargo:rustc-env=YOLO_TRAINER_VERSION={version}");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}

/// Pull the string value of the first `"version"` key. Returns `None` on malformed config so the
/// caller can fail with a message that names the file.
fn extract_version(raw: &str) -> Option<String> {
    let key = raw.find("\"version\"")?;
    let after_key = &raw[key + "\"version\"".len()..];
    let colon = after_key.find(':')?;
    let value = after_key[colon + 1..].trim_start().strip_prefix('"')?;
    let end = value.find('"')?;
    Some(value[..end].to_owned())
}
