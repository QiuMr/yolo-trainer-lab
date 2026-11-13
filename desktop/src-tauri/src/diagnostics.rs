//! Diagnostics: turn a failure into a file a developer can act on.
//!
//! The problem this solves is not "there is no feedback button". It is that a failure used to
//! leave **no trace at all**: engine stderr went to the in-memory hub, so once the window closed
//! the GPU error, the panic line, or the ONNX shape mismatch was gone. A user saying "it failed"
//! could not be helped.
//!
//! The design constraint is that **the assistant will not do extra work**. Its goal is to finish
//! the user's task; filing a bug report is not in that goal. So a report must be produced by the
//! failing request itself, and handed back inside the error response — the assistant then includes
//! it simply by relaying the error it was already going to relay.
//!
//! Redaction is on by default. A report gets sent to a stranger, so it must not carry a Windows
//! account name or the name of the user's dataset. `%USERPROFILE%` becomes `<user>` and the
//! dataset path becomes `<dataset>`; drive letters and directory depth stay, because "E: is full"
//! and "permission denied on a deep path" are exactly the failures that need them.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One report on disk, as listed by `GET /diagnostics`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportMeta {
    pub id: String,
    /// Seconds since the Unix epoch, from the id's timestamp.
    pub created_unix: u64,
    /// `YYYY-MM-DD HH:MM:SS UTC`.
    pub created: String,
    pub code: String,
    pub kind: String,
    /// First meaningful line of the report, already redacted.
    pub summary: String,
    pub path: String,
    pub bytes: u64,
    /// Whether the summary reached the configured channel. `false` does not mean the report is
    /// lost — it is on disk either way.
    pub pushed: bool,
}

/// Newly written report.
#[derive(Debug, Clone, Serialize)]
pub struct Written {
    pub id: String,
    /// `YYYY-MM-DD HH:MM:SS UTC`, for the pushed message.
    pub created: String,
    pub path: String,
    pub summary: String,
    pub bytes: u64,
}

/// What to write about. `context` is the request that failed, already reduced to the fields worth
/// keeping — dumping a whole HTTP body would carry image paths and no extra value.
pub struct Draft<'a> {
    pub kind: &'a str,
    pub code: &'a str,
    pub message: &'a str,
    pub hint: Option<&'a str>,
    pub context: Value,
}

/// `<root>/<yyyyMMdd-HHmmss>-<code>.md`.
///
/// The id is the whole filename without extension, and it is the only thing `GET /diagnostics?id=`
/// accepts. Restricting it to digits, `-` and `_` is what keeps a crafted id from walking out of
/// the directory — a path-traversal guard that needs no `canonicalize` (which fails on paths that
/// do not exist yet and adds `\\?\` on Windows).
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == '-' || ch == '_' || ch.is_ascii_lowercase())
        && id.chars().filter(|ch| *ch == '-').count() >= 2
}

/// Replace the user's home directory and a known dataset path with placeholders.
///
/// Applied to the whole rendered report rather than to individual fields, so a path that reaches
/// the log tail is redacted too — the log is written by the engine, which knows nothing about
/// redaction.
pub fn redact(text: &str, home: Option<&str>, dataset: Option<&str>) -> String {
    let mut out = text.to_owned();
    if let Some(home) = home {
        out = replace_path_forms(&out, home, "<user>");
    }
    if let Some(dataset) = dataset {
        out = replace_path_forms(&out, dataset, "<dataset>");
    }
    out
}

/// Replace one path in all the spellings it appears in inside a report.
///
/// The JSON blocks are the reason this is not a single `str::replace`: a `Value` serialized with
/// `serde_json` writes `C:\Users\me` as `C:\\Users\\me`, so matching only the raw form left the
/// user name in every context and environment block — found by reading a generated report, not by
/// a test. Forward slashes are the third form, because the engine prints paths it built itself.
fn replace_path_forms(text: &str, path: &str, replacement: &str) -> String {
    let path = path.trim_end_matches(['\\', '/']);
    // A "path" of `C:\` or shorter would match every absolute path and destroy the report.
    if path.len() <= 3 {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    let escaped = path.replace('\\', "\\\\");
    if escaped != path {
        out = replace_ci(&out, &escaped, replacement);
    }
    let slashed = path.replace('\\', "/");
    if slashed != path {
        out = replace_ci(&out, &slashed, replacement);
    }
    replace_ci(&out, path, replacement)
}

/// Windows paths are case-insensitive, and the engine may echo either casing; a case-sensitive
/// replace would silently leave the user name in the report.
fn replace_ci(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_owned();
    }
    let lower_hay = haystack.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while let Some(found) = lower_hay[cursor..].find(&lower_needle) {
        let start = cursor + found;
        out.push_str(&haystack[cursor..start]);
        out.push_str(replacement);
        cursor = start + needle.len();
    }
    out.push_str(&haystack[cursor..]);
    out
}

/// `(year, month, day)` for a Unix day count. Days-to-civil, after Howard Hinnant; no date crate is
/// worth a dependency for one timestamp.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `(YYYY-MM-DD HH:MM:SS UTC, yyyyMMdd-HHmmss)`.
pub fn stamps(unix: u64) -> (String, String) {
    let days = (unix / 86_400) as i64;
    let rem = unix % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    (
        format!("{year:04}-{month:02}-{day:02} {hh:02}:{mm:02}:{ss:02} UTC"),
        format!("{year:04}{month:02}{day:02}-{hh:02}{mm:02}{ss:02}"),
    )
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Last `limit` lines of a log, oldest first. Reads the whole file: it is capped at 8 MiB by
/// `engine::LogFile`, and seeking from the end for an arbitrary count is more code than it saves.
pub fn log_tail(path: &Path, limit: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(limit);
    lines[start..].iter().map(|line| (*line).to_owned()).collect()
}

/// Write one report and return its identity.
pub fn write(
    root: &Path,
    draft: &Draft<'_>,
    environment: &Value,
    log_tail: &[String],
    home: Option<&str>,
    dataset: Option<&str>,
) -> Result<Written, String> {
    std::fs::create_dir_all(root).map_err(|error| format!("无法创建诊断目录：{error}"))?;
    let now = now_unix();
    let (created, compact) = stamps(now);
    // The code goes into the filename, so it is stripped to a safe alphabet first. Callers pass
    // literals from a fixed set, but a filename built from an unchecked string would be a
    // path-traversal hole the moment someone adds a code derived from input.
    let safe_code: String = draft
        .code
        .chars()
        .filter(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || *ch == '_')
        .take(40)
        .collect();
    let safe_code = if safe_code.is_empty() {
        "unknown".to_owned()
    } else {
        safe_code
    };
    let id = format!("{compact}-{safe_code}");
    let path = root.join(format!("{id}.md"));

    let context = redact(&serde_json::to_string_pretty(&draft.context).unwrap_or_default(), home, dataset);
    let env = redact(&serde_json::to_string_pretty(environment).unwrap_or_default(), home, dataset);
    let logs = log_tail.join("\n");
    let logs = redact(&logs, home, dataset);

    let body = format!(
        "# 诊断报告 {id}\n\n\
         **这份文件可以直接发给开发者。** 用户名已替换为 `<user>`，数据集路径已替换为 `<dataset>`。\n\n\
         ## 概况\n\n\
         | 项 | 值 |\n| --- | --- |\n\
         | 版本 | {version} |\n\
         | 时间 | {created} |\n\
         | 类别 | {kind} |\n\
         | 错误码 | `{code}` |\n\n\
         ## 发生了什么\n\n{message}\n\n\
         ## 可能原因与建议\n\n{hint}\n\n\
         ## 运行环境\n\n```json\n{env}\n```\n\n\
         ## 请求上下文\n\n```json\n{context}\n```\n\n\
         ## 引擎日志尾部（最近 {count} 行）\n\n```\n{logs}\n```\n\n\
         ---\n生成于 YOLO Trainer {version} · 报告 id `{id}`\n",
        version = environment
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        kind = draft.kind,
        code = draft.code,
        message = draft.message.trim(),
        hint = draft.hint.unwrap_or("（无）").trim(),
        count = log_tail.len(),
    );

    std::fs::write(&path, &body).map_err(|error| format!("写入诊断报告失败：{error}"))?;

    // The summary is the one line that goes over the network, so it is built from the message
    // alone — never from context or logs, both of which can carry a dataset name.
    let summary = redact(draft.message, home, dataset)
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect::<String>();

    Ok(Written {
        id,
        created,
        path: path.to_string_lossy().into_owned(),
        summary,
        bytes: body.len() as u64,
    })
}

/// Mark a report as pushed. Rewrites one header line rather than adding an index file: an index
/// can drift from the directory, and a report is small enough that appending is cheap.
pub fn mark_pushed(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    if text.contains("\n<!-- pushed: yes -->") {
        return;
    }
    if let Some(head) = text.find("\n## 概况") {
        let mut out = String::with_capacity(text.len() + 24);
        out.push_str(&text[..head]);
        out.push_str("\n<!-- pushed: yes -->");
        out.push_str(&text[head..]);
        let _ = std::fs::write(path, out);
    }
}

/// Reports newest first. Unreadable entries are skipped rather than failing the listing: one
/// corrupt file must not hide the other nine.
pub fn list(root: &Path, limit: usize) -> Vec<ReportMeta> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<ReportMeta> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_string_lossy().into_owned();
            if !is_valid_id(&id) {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            let (date, time) = id.split_once('-')?;
            if date.len() != 8 || time.len() < 6 {
                return None;
            }
            let parse = |range: std::ops::Range<usize>| -> Option<u32> {
                text_or(date, &range)?.parse().ok()
            };
            let year: i64 = parse(0..4)? as i64;
            let month = parse(4..6)?;
            let day = parse(6..8)?;
            let hh: u32 = time[0..2].parse().ok()?;
            let mm: u32 = time[2..4].parse().ok()?;
            let ss: u32 = time[4..6].parse().ok()?;
            // Approximate inverse of `civil_from_days`; only used for ordering and display, so
            // being off by a leap second is irrelevant.
            let days = days_from_civil(year, month, day);
            let created_unix = days as u64 * 86_400
                + hh as u64 * 3600
                + mm as u64 * 60
                + ss as u64;
            let (created, _) = stamps(created_unix);
            let code = id.splitn(3, '-').nth(2).unwrap_or("unknown").to_owned();
            let kind = line_starting(&text, "| 类别 |")
                .map(|line| trim_cells(&line))
                .unwrap_or_else(|| "unknown".to_owned());
            let summary = text
                .split("## 发生了什么")
                .nth(1)
                .and_then(|rest| rest.lines().find(|line| !line.trim().is_empty()))
                .unwrap_or("")
                .trim()
                .chars()
                .take(200)
                .collect::<String>();
            Some(ReportMeta {
                id,
                created_unix,
                created,
                code,
                kind,
                summary,
                path: path.to_string_lossy().into_owned(),
                bytes: entry.metadata().ok().map(|meta| meta.len()).unwrap_or(0),
                pushed: text.contains("<!-- pushed: yes -->"),
            })
        })
        .collect();
    found.sort_by(|a, b| b.created_unix.cmp(&a.created_unix).then(b.id.cmp(&a.id)));
    found.truncate(limit.max(1));
    found
}

fn text_or<'a>(text: &'a str, range: &std::ops::Range<usize>) -> Option<&'a str> {
    text.get(range.clone())
}

fn line_starting(text: &str, prefix: &str) -> Option<String> {
    text.lines()
        .find(|line| line.trim_start().starts_with(prefix))
        .map(str::to_owned)
}

fn trim_cells(line: &str) -> String {
    line.split('|')
        .map(str::trim)
        .rfind(|cell| !cell.is_empty())
        .unwrap_or("")
        .to_owned()
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn home_directory_is_replaced_in_both_slash_styles() {
        let text = r"模型 C:\Users\13114\AppData\Roaming\x.bpk 与 C:/Users/13114/runs";
        let out = redact(text, Some(r"C:\Users\13114"), None);
        assert!(!out.contains("13114"), "user name survived: {out}");
        assert_eq!(out.matches("<user>").count(), 2);
    }

    #[test]
    fn redaction_is_case_insensitive() {
        // The engine prints lowercase drive letters on some paths; a case-sensitive match would
        // leave the account name in the report.
        let out = redact(r"c:\users\13114\data", Some(r"C:\Users\13114"), None);
        assert_eq!(out, r"<user>\data");
    }

    #[test]
    fn json_escaped_paths_are_redacted_too() {
        // Found by reading a generated report, not by a test: `serde_json` writes a backslash as
        // `\\`, so matching only the raw spelling left the user name in every JSON block.
        let json_text = r#"{"engine":"C:\\Users\\13114\\AppData\\x.exe","out":"C:\\Users\\13114\\runs"}"#;
        let out = redact(json_text, Some(r"C:\Users\13114"), None);
        assert!(!out.contains("13114"), "user name survived JSON escaping: {out}");
        assert_eq!(out.matches("<user>").count(), 2);
    }

    #[test]
    fn json_escaped_dataset_paths_are_redacted_too() {
        let json_text = r#"{"dataset":"E:\\数据\\我的集\\images"}"#;
        let out = redact(json_text, None, Some(r"E:\数据\我的集"));
        assert!(out.contains("<dataset>"), "{out}");
        assert!(!out.contains("我的集"), "{out}");
    }

    #[test]
    fn dataset_path_becomes_a_placeholder_but_the_drive_stays() {
        let out = redact(
            r"读取 E:\数据\我的 数据集\images\train 失败",
            None,
            Some(r"E:\数据\我的 数据集"),
        );
        assert!(out.contains("<dataset>"), "{out}");
        assert!(!out.contains("我的 数据集"));
    }

    #[test]
    fn a_short_home_is_left_alone() {
        // `C:\` as a "home" would replace every path prefix and destroy the report.
        assert_eq!(redact(r"C:\x", Some(r"C:\"), None), r"C:\x");
    }

    #[test]
    fn timestamps_round_trip() {
        // 2026-11-13T03:46:57Z, the v0.1.4 release.
        let (readable, compact) = stamps(1_794_541_617);
        assert_eq!(readable, "2026-11-13 03:46:57 UTC");
        assert_eq!(compact, "20261113-034657");
        assert_eq!(days_from_civil(2026, 11, 13), 1_794_541_617 / 86_400);
    }

    #[test]
    fn ids_reject_path_traversal() {
        assert!(is_valid_id("20261113-034657-engine_spawn_failed"));
        for bad in ["../secret", "..\\..\\x", "a/b", "C:evil", "", "20261113-034657-../../x"] {
            assert!(!is_valid_id(bad), "accepted {bad:?}");
        }
    }

    #[test]
    fn report_carries_env_context_and_log_tail() {
        let root = std::env::temp_dir().join(format!("yt-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let draft = Draft {
            kind: "train_start_failed",
            code: "engine_spawn_failed",
            message: "引擎进程退出，代码 1",
            hint: Some("把显存降到 4GB 以下"),
            context: json!({ "epochs": 3, "dataset": r"E:\数据\mine" }),
        };
        let env = json!({ "version": "0.1.4", "adapter": "Device<Flex(Cpu)>" });
        let written = write(
            &root,
            &draft,
            &env,
            &["line one".to_owned(), "line two".to_owned()],
            Some(r"C:\Users\13114"),
            Some(r"E:\数据\mine"),
        )
        .unwrap();
        let body = std::fs::read_to_string(&written.path).unwrap();
        assert!(body.contains("0.1.4"));
        assert!(body.contains("engine_spawn_failed"));
        assert!(body.contains("line two"));
        assert!(body.contains("<dataset>"), "context path not redacted");
        assert!(!body.contains(r"E:\数据\mine"));
        // Summary is what goes over the wire: one line, no path.
        assert_eq!(written.summary, "引擎进程退出，代码 1");
        let listed = list(&root, 10);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, "train_start_failed");
        assert!(!listed[0].pushed);
        mark_pushed(&std::path::PathBuf::from(&written.path));
        assert!(list(&root, 10)[0].pushed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_directory_lists_nothing_instead_of_failing() {
        assert!(list(&std::env::temp_dir().join("yt-diag-does-not-exist"), 5).is_empty());
    }

    #[test]
    fn log_tail_keeps_the_last_lines() {
        let path = std::env::temp_dir().join(format!("yt-log-{}.log", std::process::id()));
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(log_tail(&path, 2), vec!["c", "d"]);
        assert_eq!(log_tail(&path, 99).len(), 4);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_long_summary_is_cut_not_copied_whole() {
        let root = std::env::temp_dir().join(format!("yt-diag-long-{}", std::process::id()));
        let long = "x".repeat(500);
        let draft = Draft {
            kind: "k",
            code: "c",
            message: &long,
            hint: None,
            context: json!({}),
        };
        let written = write(&root, &draft, &json!({}), &[], None, None).unwrap();
        assert_eq!(written.summary.len(), 200);
        let _ = std::fs::remove_dir_all(&root);
    }
}
