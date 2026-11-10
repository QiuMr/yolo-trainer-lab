//! Single-image inference.
//!
//! The engine's `predict` subcommand prints three things to one stream: a `Loaded ... with Burn.`
//! line, a pretty-printed JSON document, and a `Top-1: ...` / table summary. So "parse stdout as
//! JSON" does not work, and neither does "parse each line" — the document spans lines. The only
//! reliable approach is to find the first `{` that begins a complete JSON value and stop there.
//!
//! The output image is always written to an app-owned directory. The engine's default is
//! `<source-stem>-detections.png` **next to the source image**, which silently pollutes a dataset
//! directory — the exact failure this project already hit once.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::engine::hidden_command;

#[derive(Debug, Clone, Deserialize)]
pub struct PredictRequest {
    /// Montgomery `.bpk`. Omitted means "the best model from the most recent run".
    #[serde(default)]
    pub model: Option<String>,
    /// Image file: JPEG, PNG, or WebP.
    pub source: String,
    /// Where to write the annotated image. Defaults to the app's `predictions/` directory.
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
    /// Render instance masks. Only meaningful for `*-seg` models.
    #[serde(default)]
    pub masks: bool,
    #[serde(default)]
    pub device: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PredictResult {
    pub model: String,
    pub source: String,
    /// Absent for classification models, which produce no annotated image.
    pub output: Option<String>,
    /// The engine's own document, passed through unchanged: its shape is the engine's contract
    /// (docs/API.md), not something this layer should re-model.
    pub prediction: serde_json::Value,
}

/// Pull the first complete JSON value out of a stream that also carries human text.
fn extract_json(stdout: &str) -> Option<serde_json::Value> {
    for (index, _) in stdout.match_indices('{') {
        let mut stream =
            serde_json::Deserializer::from_str(&stdout[index..]).into_iter::<serde_json::Value>();
        if let Some(Ok(value)) = stream.next() {
            return Some(value);
        }
    }
    None
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_owned())
}

/// Run inference and hand back both the structured prediction and where the picture landed.
pub fn run(
    engine: &Path,
    request: &PredictRequest,
    model: &Path,
    default_dir: &Path,
) -> Result<PredictResult, String> {
    let source = PathBuf::from(request.source.trim());
    if !source.is_file() {
        return Err(format!("图片不存在：{}", source.display()));
    }

    let output = match request.output.as_deref() {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => {
            std::fs::create_dir_all(default_dir)
                .map_err(|error| format!("创建输出目录失败：{error}"))?;
            default_dir.join(format!("{}-predicted.png", stem_of(&source)))
        }
    };
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建输出目录失败：{error}"))?;
    }

    let mut command = hidden_command(engine);
    command
        .arg("predict")
        .arg("--model")
        .arg(model)
        .arg("--source")
        .arg(&source)
        .arg("--output")
        .arg(&output)
        .arg("--json")
        .arg("--device")
        .arg(request.device.as_deref().unwrap_or("cpu"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if let Some(confidence) = request.confidence {
        if !(0.0..=1.0).contains(&confidence) {
            return Err("confidence 必须在 0 到 1 之间。".into());
        }
        command.arg("--confidence").arg(confidence.to_string());
    }
    if request.masks {
        command.arg("--masks");
    }

    let result = command
        .output()
        .map_err(|error| format!("推理进程启动失败：{error}"))?;
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    if !result.status.success() {
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            stdout.trim()
        } else {
            detail
        };
        return Err(format!("推理失败：{detail}"));
    }

    let prediction = extract_json(&stdout).ok_or_else(|| {
        let tail = stdout.lines().rev().take(4).collect::<Vec<_>>().join(" / ");
        format!("推理没有返回 JSON：{tail}")
    })?;

    // The engine writes no image for classification, so reporting a path there would be a lie.
    let wrote_image = output.is_file();
    Ok(PredictResult {
        model: model.to_string_lossy().into_owned(),
        source: source.to_string_lossy().into_owned(),
        output: wrote_image.then(|| output.to_string_lossy().into_owned()),
        prediction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "Loaded C:/runs/best.bpk (yolo26n-cls) with Burn.\n\
{\n  \"task\": \"classification\",\n  \"classes\": [\n    {\"class_id\": 2, \"class_name\": \"triangle\", \"confidence\": 0.334}\n  ]\n}\n\
Top-1: triangle (33.42%)\n";

    /// The exact shape the engine emits: human text, a multi-line document, human text.
    #[test]
    fn extracts_json_between_human_lines() {
        let value = extract_json(REAL).expect("json");
        assert_eq!(value["task"], "classification");
        assert_eq!(value["classes"][0]["class_name"], "triangle");
    }

    #[test]
    fn extracts_pure_json() {
        let value = extract_json("{\"task\":\"detection\"}").unwrap();
        assert_eq!(value["task"], "detection");
    }

    /// A stray brace in a log line must not be mistaken for the document.
    #[test]
    fn skips_a_brace_that_starts_nothing() {
        let messy = "warning: expected { but found\n{\"task\":\"detection\",\"detections\":[]}";
        let value = extract_json(messy).unwrap();
        assert_eq!(value["task"], "detection");
    }

    #[test]
    fn returns_none_when_there_is_no_document() {
        assert!(extract_json("error: model failed to load").is_none());
    }
}
