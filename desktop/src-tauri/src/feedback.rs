//! Push a redacted summary to whichever channel the developer configured.
//!
//! Channels are chosen because they are reachable from mainland China without a VPN and cost
//! nothing: a WeCom group robot needs only a webhook URL and one free app, PushPlus needs only a
//! token from a WeChat login scan. GitHub Issues was the obvious first idea and the wrong one —
//! the people who hit these bugs are not going to create a GitHub account to report one.
//!
//! Only the one-line summary and the error code leave the machine. Paths, dataset names and log
//! tails stay in the local report (`diagnostics`), which the user can open, edit, and decide to
//! send.
//!
//! Failures here are never fatal and never user-visible beyond a `pushed: false` field: the run
//! already failed, and a broken webhook must not turn into a second failure.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::json;

/// One report per failure, and a failure storm can realistically produce several at once (spawn
/// fails, every client retries, each retry is another 500). The channel is a group chat receiving
/// phone notifications, so flooding it would get the bot muted — which is worse than losing the
/// third duplicate of the same error.
const MAX_PER_MINUTE: usize = 3;
const WINDOW: Duration = Duration::from_secs(60);

/// A dead webhook host must not hold the error response for the OS default timeout.
const TIMEOUT: Duration = Duration::from_secs(5);

/// One message cap for WeCom markdown, with headroom for the prefix this module adds.
const MAX_BODY_BYTES: usize = 4000;

static RECENT: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

/// True when another push is allowed, consuming a slot.
///
/// Deliberately checks-and-inserts under one lock: two threads deciding separately would both see
/// room and both send, which is exactly the case this exists to prevent.
fn allow(now: Instant) -> bool {
    let Ok(mut recent) = RECENT.lock() else {
        // A poisoned lock means a previous push panicked mid-lock. Not sending is the safe side.
        return false;
    };
    recent.retain(|at| now.duration_since(*at) < WINDOW);
    if recent.len() >= MAX_PER_MINUTE {
        return false;
    }
    recent.push(now);
    true
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_BODY_BYTES {
        return text.to_owned();
    }
    // Cut on a char boundary; a byte slice through a multi-byte character would panic.
    let mut end = MAX_BODY_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…（已截断）", &text[..end])
}

/// Send `markdown` to `target`.
///
/// `kind` selects the wire format: `wecom` (a group-robot webhook URL), `pushplus` (a token), or
/// anything else, which is treated as `generic`: a plain POST of the summary as JSON.
pub fn push(kind: &str, target: &str, title: &str, markdown: &str, id: &str) -> Result<(), String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("没有配置上报地址".into());
    }
    if !allow(Instant::now()) {
        return Err("一分钟内已推送 3 条，本次跳过".into());
    }

    // Same panic as the weights downloader: `rustls-no-provider` supplies no backend until the
    // application installs one, and the panic surfaces on a worker thread the caller cannot see.
    crate::pretrained::install_crypto_provider();

    let body = clip(markdown);
    let (url, payload) = match kind {
        "pushplus" => (
            "https://www.pushplus.plus/send".to_owned(),
            json!({
                "token": target,
                "title": title,
                "content": body,
                "template": "markdown",
            }),
        ),
        "generic" => (
            target.to_owned(),
            json!({ "source": "yolo-trainer", "id": id, "title": title, "summary": body }),
        ),
        // WeCom group robot: the URL already carries the key.
        _ => (
            target.to_owned(),
            json!({ "msgtype": "markdown", "markdown": { "content": body } }),
        ),
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(TIMEOUT)
        .build()
        .map_err(|error| format!("初始化上报通道失败：{error}"))?;
    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .map_err(|error| format!("上报失败：{error}"))?;
    let status = response.status();

    // WeCom answers 200 with an `errcode` on failure (bad key, rate limit, oversized body). Trusting
    // the status code would report "已推送" for a message nobody received.
    let rejected = response
        .text()
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| {
            let code = value.get("errcode").and_then(serde_json::Value::as_i64)?;
            if code == 0 {
                return None;
            }
            let message = value
                .get("errmsg")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("未知错误");
            Some(format!("上报被拒绝（errcode {code}）：{message}"))
        });
    if let Some(reason) = rejected {
        return Err(reason);
    }
    if !status.is_success() {
        return Err(format!("上报失败，HTTP {status}"));
    }
    Ok(())
}

/// The message body. Kept here so the shape is testable without a network.
pub fn compose(version: &str, created: &str, code: &str, summary: &str, id: &str, env: &str) -> String {
    format!(
        "## YOLO Trainer 报错\n\
         > {version} · {created}\n\n\
         **错误码**：`{code}`\n\n\
         **现象**：{summary}\n\n\
         **环境**：{env}\n\n\
         **报告 id**：`{id}`\n\n\
         完整报告在这台机器上（`diagnostics/{id}.md`），路径与数据集名未上传。\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_only_allows_three() {
        // Reset first: other tests in this binary may have consumed slots.
        if let Ok(mut recent) = RECENT.lock() {
            recent.clear();
        }
        let now = Instant::now();
        assert!(allow(now));
        assert!(allow(now));
        assert!(allow(now));
        assert!(!allow(now), "fourth push inside one minute must be refused");
        // A minute later the window is empty again.
        assert!(allow(now + WINDOW + Duration::from_secs(1)));
        if let Ok(mut recent) = RECENT.lock() {
            recent.clear();
        }
    }

    #[test]
    fn an_empty_target_is_refused_before_any_network_call() {
        assert!(push("wecom", "   ", "t", "b", "id").is_err());
    }

    #[test]
    fn an_over_long_body_is_cut_on_a_character_boundary() {
        let text = "中".repeat(5000);
        let out = clip(&text);
        assert!(out.ends_with("（已截断）"));
        // Would have panicked on a bad boundary, and must not be larger than the cap + suffix.
        assert!(out.len() <= MAX_BODY_BYTES + 32);
    }

    #[test]
    fn the_message_carries_code_and_summary_but_no_path() {
        let body = compose(
            "0.1.4",
            "2026-11-13 04:00:00 UTC",
            "engine_spawn_failed",
            "引擎进程退出，代码 1",
            "20261113-040000-engine_spawn_failed",
            "Device<Flex(Cpu)> · windows",
        );
        assert!(body.contains("engine_spawn_failed"));
        assert!(body.contains("Device<Flex(Cpu)>"));
        assert!(!body.contains(r"C:\"));
    }

    /// A local HTTP server stands in for the group robot: the interesting claim is "a request is
    /// really sent, with the shape WeCom expects", not "WeCom accepts it".
    #[test]
    fn a_wecom_push_really_posts_the_markdown_payload() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .ok();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            // One read is not guaranteed to hold the whole body; read until the declared length
            // is satisfied, otherwise the assertion below can race.
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&raw);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let declared = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + declared {
                        break;
                    }
                }
            }
            let body = br#"{"errcode":0,"errmsg":"ok"}"#;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(body);
            let _ = stream.flush();
            String::from_utf8_lossy(&raw).into_owned()
        });

        if let Ok(mut recent) = RECENT.lock() {
            recent.clear();
        }
        let result = push(
            "wecom",
            &format!("http://127.0.0.1:{port}/cgi-bin/webhook/send?key=test"),
            "YOLO Trainer 0.1.4 engine_spawn_failed",
            "## YOLO Trainer 报错\n**错误码**：`engine_spawn_failed`",
            "20261113-040000-engine_spawn_failed",
        );
        assert!(result.is_ok(), "push should succeed against a 200 + errcode 0: {result:?}");

        let request = server.join().expect("server thread");
        assert!(request.starts_with("POST "), "{request}");
        assert!(request.contains("/cgi-bin/webhook/send?key=test"), "{request}");
        // The body must be the robot's own shape, not a generic webhook dump.
        assert!(request.contains(r#""msgtype":"markdown""#), "{request}");
        assert!(request.contains("engine_spawn_failed"), "{request}");
        if let Ok(mut recent) = RECENT.lock() {
            recent.clear();
        }
    }

    /// WeCom answers 200 with a non-zero `errcode` when the key is wrong. Trusting the status code
    /// would report "已推送" for a message that reached nobody.
    #[test]
    fn a_non_zero_errcode_is_a_failure_even_on_http_200() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut chunk = [0u8; 4096];
            let _ = stream.read(&mut chunk);
            let body = br#"{"errcode":93000,"errmsg":"invalid webhook url"}"#;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(body);
            let _ = stream.flush();
        });

        if let Ok(mut recent) = RECENT.lock() {
            recent.clear();
        }
        let result = push(
            "wecom",
            &format!("http://127.0.0.1:{port}/send?key=bad"),
            "t",
            "b",
            "id",
        );
        let Err(message) = result else {
            panic!("a rejected key must not look like success");
        };
        assert!(message.contains("93000"), "{message}");
        if let Ok(mut recent) = RECENT.lock() {
            recent.clear();
        }
    }
}
