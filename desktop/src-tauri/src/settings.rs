//! Persisted user settings.
//!
//! One JSON file in the app config directory. Every field is optional-with-default so a file
//! written by an older build still loads: losing a preference is fine, refusing to start is not.
//!
//! Nothing here validates that the configured engine path actually exists — the settings view
//! reports that separately. Blocking the save would mean a typo in one field makes every other
//! field unsaveable, which is worse than a visible "找不到引擎" line.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Values `--device` accepts. Kept in sync with montgomery's `DeviceSelection`.
pub const DEVICES: [&str; 3] = ["auto", "cpu", "gpu"];

/// Coerce a requested device into one `--device` accepts, falling back when it is blank.
///
/// This is the **single source** of the rule. Both the engine spawn path and the HTTP control
/// service call it: when the control service had its own copy, `POST /train` echoed the requested
/// string while the engine ran something else, so an AI that asked for `gpu` and silently got
/// `auto` had no way to notice.
pub fn normalize_device(requested: &str, fallback: &str) -> String {
    let requested = requested.trim();
    if requested.is_empty() {
        return normalize_device(fallback, "auto");
    }
    if DEVICES.contains(&requested) {
        return requested.to_owned();
    }
    "auto".to_owned()
}


/// Default port for the local control service. Chosen from the dynamic range and far away from
/// the well-known ports a developer machine is likely to have occupied (3000, 5000, 8080).
pub const DEFAULT_CONTROL_PORT: u16 = 47821;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Explicit path to `montgomery.exe`; `None` means auto-detect.
    pub engine_path: Option<String>,
    /// One of [`DEVICES`]. Used as the default on the training page.
    pub default_device: String,
    /// Root for `runs/`. `None` means the app data directory.
    pub output_dir: Option<String>,
    /// Whether the AI-facing HTTP service listens. On by default: it binds loopback only and
    /// needs a bearer token, and a service the user has to discover before it works is a service
    /// nobody uses.
    pub control_enabled: bool,
    /// `0` asks the OS for a free port; the actual port is reported back in `SettingsView`.
    pub control_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine_path: None,
            default_device: "auto".into(),
            output_dir: None,
            control_enabled: true,
            control_port: DEFAULT_CONTROL_PORT,
        }
    }
}

impl Settings {
    /// Trim blanks to `None` and coerce an unknown device to `auto`.
    ///
    /// The UI sends `""` for "not set", and an unknown device string would make the engine exit
    /// with a clap error that no user can act on.
    #[must_use]
    pub fn normalize(mut self) -> Self {
        self.engine_path = self
            .engine_path
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        self.output_dir = self
            .output_dir
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if !DEVICES.contains(&self.default_device.as_str()) {
            self.default_device = "auto".into();
        }
        self
    }

    pub fn output_dir_or(&self, fallback: &Path) -> PathBuf {
        let chosen = match self.output_dir.as_deref() {
            Some(explicit) => PathBuf::from(explicit),
            None => fallback.to_path_buf(),
        };
        // A relative path resolves against the *install directory* in a packaged build, so the
        // user would see "runs/" in settings while the files land somewhere unfindable.
        // `absolute` (unlike canonicalize) works before the directory exists and adds no \\?\
        // prefix on Windows — the same choice dataset.rs makes for manifests.
        std::path::absolute(&chosen).unwrap_or(chosen)
    }
}

/// File-backed settings. `load` never fails: a corrupt file silently becomes the defaults, and the
/// next save overwrites it.
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Settings {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
            .unwrap_or_default()
            .normalize()
    }

    pub fn save(&self, settings: &Settings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建配置目录：{error}"))?;
        }
        let text = serde_json::to_string_pretty(settings)
            .map_err(|error| format!("配置序列化失败：{error}"))?;
        std::fs::write(&self.path, text).map_err(|error| format!("写入配置失败：{error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_fall_back_to_defaults() {
        // A config from a build that had a field this one does not know must still load.
        let parsed: Settings = serde_json::from_str(r#"{"default_device":"gpu"}"#).unwrap();
        assert!(parsed.engine_path.is_none());
        assert_eq!(parsed.normalize().default_device, "gpu");
    }

    #[test]
    fn normalize_rejects_junk() {
        let mess = Settings {
            engine_path: Some("   ".into()),
            default_device: "tpu".into(),
            output_dir: Some(String::new()),
            ..Settings::default()
        }
        .normalize();
        assert!(mess.engine_path.is_none());
        assert!(mess.output_dir.is_none());
        // An unknown device must not reach the engine: clap would abort the run.
        assert_eq!(mess.default_device, "auto");
    }
}
