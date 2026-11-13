//! Pretrained weights, fetched on demand instead of shipped.
//!
//! # Why not bundle them
//!
//! A `.bpk` is real half-precision weights and compresses at 1.11x, so the 15 detection models
//! alone are ~374 MiB — twenty times the current download. The ONNX skeletons compress 355x
//! because they carry no values at all; this is the opposite case. Bundling would end the
//! "installer measured in tens of megabytes" claim for a feature most users touch once.
//!
//! So the installer carries only this manifest (`arch`, size, sha256 — kilobytes), and each
//! weight is fetched from a GitHub Release the first time someone actually trains with it. That
//! also uncaps the catalog: offering s/m/l/x costs the installer nothing, and the user pays only
//! for the rung they pick.
//!
//! # Offline is a normal state
//!
//! No network, a slow proxy and GitHub being unreachable are ordinary conditions, not broken app
//! states. Every failure message names the file and the exact directory it belongs in, so a user
//! behind a blocked network can place it by hand and continue. `cached_path` accepts a weight that
//! was put there manually as long as its digest matches — which is also what stops a truncated
//! download from being trained on.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

/// Progress events while downloading. Consumed by the train page's weight picker.
pub const PROGRESS_EVENT: &str = "pretrained-progress";

/// Compiled in rather than read from disk: a missing resource file would produce the worst kind of
/// failure — the app launches normally and then silently offers no weights. A build either has the
/// manifest or does not exist.
const MANIFEST: &str = include_str!("../resources/pretrained/manifest.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    #[allow(dead_code)]
    pub version: u32,
    /// Directory-style prefix ending in `/`. Every entry's `file` is appended to it.
    pub base_url: String,
    pub weights: Vec<Entry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub arch: String,
    pub family: String,
    pub scale: String,
    pub task: String,
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
    pub classes: usize,
    /// What the weights were pretrained on (`COCO`, `ImageNet`). Shown so a user understands why a
    /// brand-new model already recognises things.
    pub dataset: String,
}

/// One row for the picker. Deliberately carries no URL: the window has no business building one,
/// and a URL in the payload invites it to bypass the digest check.
#[derive(Debug, Clone, Serialize)]
pub struct WeightView {
    pub arch: String,
    pub family: String,
    pub scale: String,
    pub task: String,
    pub bytes: u64,
    pub classes: usize,
    pub dataset: String,
    pub cached: bool,
    /// Present only when the bytes are already on disk and verified-or-locatable.
    pub path: Option<String>,
}

#[derive(Clone, Serialize)]
struct Progress<'a> {
    arch: &'a str,
    phase: &'a str,
    received: u64,
    total: u64,
}

pub fn manifest() -> Manifest {
    serde_json::from_str(MANIFEST).expect("bundled pretrained manifest is not valid JSON")
}

/// Where downloaded weights live. `app_local_data_dir` rather than the install directory: Program
/// Files is read-only for a normal user, and a weight is user data, not part of the app.
pub fn weights_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("找不到应用数据目录：{error}"))?
        .join("weights");
    fs::create_dir_all(&dir).map_err(|error| format!("创建权重目录失败：{error}"))?;
    Ok(dir)
}

/// Directories that may hold weights without a download: the cache first, then the repository
/// checkout during development (where `resources/pretrained/*.bpk` is gitignored, so it never
/// reaches an installer — it only saves the developer the download).
fn candidate_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = weights_dir(app) {
        dirs.push(dir);
    }
    if let Ok(resources) = crate::onnx_export::resolve_resource("pretrained") {
        dirs.push(resources);
    }
    dirs
}

fn locate(app: &AppHandle, entry: &Entry) -> Option<PathBuf> {
    candidate_dirs(app)
        .into_iter()
        .map(|dir| dir.join(&entry.file))
        .find(|path| path.is_file() && verify(path, entry).is_ok())
}

/// Every weight the build knows about, annotated with whether it is already available locally.
pub fn catalog(app: &AppHandle) -> Vec<WeightView> {
    manifest()
        .weights
        .into_iter()
        .map(|entry| {
            let path = locate(app, &entry);
            WeightView {
                cached: path.is_some(),
                path: path.map(|value| value.to_string_lossy().into_owned()),
                arch: entry.arch,
                family: entry.family,
                scale: entry.scale,
                task: entry.task,
                bytes: entry.bytes,
                classes: entry.classes,
                dataset: entry.dataset,
            }
        })
        .collect()
}

fn verify(path: &Path, entry: &Entry) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("读不到 {}：{error}", path.display()))?;
    if metadata.len() != entry.bytes {
        return Err(format!(
            "文件大小不符：期望 {} 字节，实际 {} 字节",
            entry.bytes,
            metadata.len()
        ));
    }
    let mut file = File::open(path).map_err(|error| format!("打不开 {}：{error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let actual = digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    if actual != entry.sha256 {
        return Err("校验不通过：文件内容与清单不符（可能下载被截断）".to_owned());
    }
    Ok(())
}

/// Install the TLS crypto backend, once per process.
///
/// `reqwest` is built with `rustls-no-provider` (the feature set the updater plugin already uses, so
/// no extra crate is pulled in), which means nothing supplies a `CryptoProvider` until the
/// application does. Without this, building the first client panics inside reqwest's worker thread —
/// and because that thread is not the caller's, the panic surfaces as the unhelpful
/// "event loop thread panicked".
pub(crate) fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // An error here means someone else installed one first, which is equally fine: all that
        // matters is that a provider exists before the client is built.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Return a usable path for `arch`, downloading it first if it is not on disk.
///
/// Blocking: called from a dedicated thread because `reqwest`'s blocking client builds its own
/// runtime. Emits [`PROGRESS_EVENT`] as bytes arrive so the window can show movement on a 130 MiB
/// fetch instead of looking frozen.
pub fn ensure(app: &AppHandle, arch: &str) -> Result<String, String> {
    let manifest = manifest();
    let entry = manifest
        .weights
        .iter()
        .find(|entry| entry.arch == arch)
        .ok_or_else(|| format!("「{arch}」没有可下载的预训练权重"))?;

    if let Some(path) = locate(app, entry) {
        return Ok(path.to_string_lossy().into_owned());
    }

    let dir = weights_dir(app)?;
    let url = format!("{}{}", manifest.base_url, entry.file);
    let mut report = |phase, received, total| {
        let _ = app.emit(PROGRESS_EVENT, Progress { arch, phase, received, total });
    };
    let path = fetch(&url, entry, &dir, &mut report)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Download `entry` into `dir`, verify it, and return the final path.
///
/// Split out from [`ensure`] so the transfer can be exercised without a Tauri app handle — the
/// network test below drives exactly this function.
fn fetch(
    url: &str,
    entry: &Entry,
    dir: &Path,
    report: &mut dyn FnMut(&'static str, u64, u64),
) -> Result<PathBuf, String> {
    let final_path = dir.join(&entry.file);
    let partial = dir.join(format!("{}.part", entry.file));
    fs::create_dir_all(dir).map_err(|error| format!("创建权重目录失败：{error}"))?;

    report("download", 0, entry.bytes);
    install_crypto_provider();

    let agent = reqwest::blocking::Client::builder()
        // Only the connect phase is bounded. A whole-request timeout would kill the 130 MiB
        // weights on a slow but working link, which is the common case on this network.
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("初始化下载器失败：{error}"))?;
    let response = agent.get(url).send().map_err(|error| {
        format!(
            "下载失败：{error}。如果网络访问不了 GitHub，可以把 {} 手动放到 {}，再点一次开始训练。",
            entry.file,
            dir.display()
        )
    })?;
    if !response.status().is_success() {
        return Err(format!(
            "下载失败：服务器返回 {}。这个权重可能还没发布，或者网络被拦截。",
            response.status()
        ));
    }

    let total = response.content_length().unwrap_or(entry.bytes);

    // `reqwest::blocking::Response` is a `Read`, which is exactly why the blocking client is worth
    // the extra dependency: the body is consumed in chunks here so progress is per megabyte.
    let mut reader = response;
    let mut writer = BufWriter::new(
        File::create(&partial).map_err(|error| format!("创建临时文件失败：{error}"))?,
    );
    let mut buffer = [0u8; 128 * 1024];
    let mut received = 0u64;
    let mut last_emit = 0u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| {
                let _ = fs::remove_file(&partial);
                format!("下载中断：{error}")
            })?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("写入失败：{error}"))?;
        received += read as u64;
        // Once per megabyte is enough to look alive and keeps the event channel cheap on a
        // 130 MiB transfer (1280 events instead of ~100k).
        if received - last_emit >= 1024 * 1024 {
            last_emit = received;
            report("download", received, total);
        }
    }
    writer.flush().map_err(|error| format!("写入失败：{error}"))?;
    drop(writer);

    report("verify", received, total);

    if let Err(reason) = verify(&partial, entry) {
        let _ = fs::remove_file(&partial);
        return Err(format!("{reason}。已删除不完整的文件，请重试。"));
    }
    fs::rename(&partial, &final_path).map_err(|error| format!("保存权重失败：{error}"))?;

    report("done", received, total);
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_manifest_parses_and_is_internally_consistent() {
        let manifest = manifest();
        assert!(!manifest.weights.is_empty(), "a build with no weights offers nothing");
        assert!(manifest.base_url.starts_with("https://"), "plain http would be interceptable");
        assert!(manifest.base_url.ends_with('/'), "the file name is appended verbatim");
        for entry in &manifest.weights {
            assert_eq!(
                entry.sha256.len(),
                64,
                "{} carries something that is not a sha256",
                entry.arch
            );
            assert!(entry.bytes > 0, "{} declares no size", entry.arch);
            assert!(entry.file.ends_with(".bpk"), "{} is not a burnpack", entry.arch);
            assert!(
                matches!(entry.task.as_str(), "detect" | "segment" | "classify"),
                "{} has an unknown task",
                entry.arch
            );
        }
    }

    #[test]
    fn architecture_names_are_unique() {
        let manifest = manifest();
        let mut seen = std::collections::HashSet::new();
        for entry in &manifest.weights {
            assert!(seen.insert(entry.arch.clone()), "{} appears twice", entry.arch);
        }
    }

    #[test]
    fn a_truncated_file_fails_verification_instead_of_being_trained_on() {
        let dir = std::env::temp_dir().join("yolo-pretrained-verify");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("fake.bpk");
        let data = b"not the real bytes";
        fs::write(&path, data).unwrap();
        let entry = Entry {
            arch: "yolov8n".into(),
            family: "yolov8".into(),
            scale: "n".into(),
            task: "detect".into(),
            file: "fake.bpk".into(),
            bytes: 4,
            sha256: "00".repeat(32),
            classes: 80,
            dataset: "COCO".into(),
        };
        let error = verify(&path, &entry).unwrap_err();
        assert!(error.contains("文件大小不符"), "{error}");

        // Right size, wrong content — the case a length check alone would wave through.
        let entry = Entry { bytes: data.len() as u64, ..entry };
        let error = verify(&path, &entry).unwrap_err();
        assert!(error.contains("校验不通过"), "{error}");
        let _ = fs::remove_file(&path);
    }

    /// Real network. Run deliberately: `cargo test --lib -- --ignored --nocapture`.
    ///
    /// Everything else in this module is offline, but the one thing worth knowing before shipping —
    /// that a Release asset is actually reachable and matches its digest — can only be learned by
    /// fetching it. It picks the smallest weight, which is also the rung a first-time user is most
    /// likely to start with.
    #[test]
    #[ignore = "downloads from GitHub; run deliberately"]
    fn downloads_and_verifies_a_real_weight() {
        let manifest = manifest();
        let entry = manifest
            .weights
            .iter()
            .min_by_key(|entry| entry.bytes)
            .expect("the manifest is not empty");
        let dir = std::env::temp_dir().join("yolo-pretrained-network");
        let _ = fs::remove_dir_all(&dir);
        let url = format!("{}{}", manifest.base_url, entry.file);

        let mut phases: Vec<&'static str> = Vec::new();
        let path = {
            let mut report = |phase: &'static str, _: u64, _: u64| phases.push(phase);
            fetch(&url, entry, &dir, &mut report).unwrap()
        };

        assert!(path.is_file(), "{} was not written", path.display());
        verify(&path, entry).unwrap();
        assert_eq!(
            *phases.last().unwrap(),
            "done",
            "the last event must be the terminal one: {phases:?}"
        );
        assert!(phases.contains(&"download") && phases.contains(&"verify"), "{phases:?}");
        // Verification of what actually arrived on disk, not of the buffer we just wrote.
        assert_eq!(fs::metadata(&path).unwrap().len(), entry.bytes);
        println!("{} verified at {}", entry.arch, path.display());
        let _ = fs::remove_dir_all(&dir);
    }
}
