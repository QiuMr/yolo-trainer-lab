//! Offline ONNX export.
//!
//! The graph is not built here and no Python is involved: the installer ships one skeleton per
//! supported architecture (structure only, a few hundred kilobytes) and the engine writes the
//! trained weights into it. This module only decides *where* the pieces are and reports what came
//! out, because the window cannot guess the resource layout between `tauri dev` and an installed
//! build.
//!
//! Two things are deliberately enforced:
//! * the result always lands inside the app's own output directory, never next to the checkpoint —
//!   the same rule as inference, for the same reason (writing into a run directory silently
//!   corrupts the next scan);
//! * an architecture without a bundled skeleton fails with a sentence naming what is missing,
//!   instead of producing an ONNX whose graph does not match the weights.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::engine::hidden_command;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExportRequest {
    /// Montgomery `.bpk`. Omitted means "the best model from the most recent run".
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub output: String,
    pub classes: u64,
    pub tensors: u64,
    pub bytes: u64,
}

/// Locate the bundled skeletons.
///
/// The candidates cover the three layouts this app actually runs in: an installed build (resources
/// next to the executable), `tauri dev` (the working directory is `src-tauri`), and a plain
/// `cargo run` from the repository root.
pub fn resolve_templates() -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Ok(explicit) = std::env::var("MONTGOMERY_ONNX_TEMPLATES") {
        candidates.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("onnx"));
            candidates.push(dir.join("resources").join("onnx"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("resources").join("onnx"));
        candidates.push(cwd.join("src-tauri").join("resources").join("onnx"));
        if let Some(root) = cwd.parent() {
            candidates.push(root.join("src-tauri").join("resources").join("onnx"));
        }
    }
    for candidate in &candidates {
        if candidate.is_dir() {
            return Ok(candidate.clone());
        }
    }
    Err(format!(
        "没有找到 ONNX 骨架目录，已查找：{}。安装包损坏或不是官方构建。",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("；")
    ))
}

/// Name the artifact after both its run and its file.
///
/// `<run>/exports/best.bpk` flattened into one directory would have every run overwrite the same
/// `best.onnx`, so the run name stays in the filename.
fn output_name(model: &Path) -> String {
    let stem = model
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".to_owned());
    let run = model
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    if run.is_empty() {
        format!("{stem}.onnx")
    } else {
        format!("{run}-{stem}.onnx")
    }
}

pub fn run(
    engine: &Path,
    model: &Path,
    templates: &Path,
    output_dir: &Path,
) -> Result<ExportResult, String> {
    std::fs::create_dir_all(output_dir)
        .map_err(|error| format!("创建导出目录失败：{error}"))?;
    let output = output_dir.join(output_name(model));

    let mut command = hidden_command(engine);
    command
        .arg("export")
        .arg("--model")
        .arg(model)
        .arg("--format")
        .arg("onnx")
        .arg("--output")
        .arg(&output)
        .arg("--template")
        .arg(templates)
        // Exporting twice must work: the button is repeatable, and the file is app-owned.
        .arg("--force")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let result = command
        .output()
        .map_err(|error| format!("导出进程启动失败：{error}"))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        let detail = if stderr.trim().is_empty() { stdout } else { stderr };
        return Err(format!("导出失败：{}", detail.trim()));
    }

    let bytes = std::fs::metadata(&output)
        .map_err(|error| format!("导出结束但文件读不到：{error}"))?
        .len();
    // The engine writes a sidecar describing what it did; read it rather than parsing log output.
    let mut sidecar = output.clone().into_os_string();
    sidecar.push(".json");
    let record: serde_json::Value = std::fs::read_to_string(Path::new(&sidecar))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);

    Ok(ExportResult {
        output: output.to_string_lossy().into_owned(),
        classes: record["classes"].as_u64().unwrap_or(0),
        tensors: record["tensors"].as_u64().unwrap_or(0),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_name_keeps_the_run_so_exports_do_not_collide() {
        let path = Path::new("C:/runs/train-1700000000-0000abcd/exports/best.bpk");
        assert_eq!(
            output_name(path),
            "train-1700000000-0000abcd-best.onnx",
            "the run directory is what separates today's model from yesterday's"
        );
        assert_eq!(output_name(Path::new("C:/models/custom.bpk")), "custom.onnx");
    }
}
