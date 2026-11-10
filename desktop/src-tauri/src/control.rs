//! Local HTTP control service — the "let an AI drive this app" surface.
//!
//! Design constraints, and why this is hand-written rather than `axum` or `tiny_http`:
//!
//! * The whole application is blocking `std::thread`; it has no async runtime and does not want
//!   one just to answer four GETs. See DECISIONS D017 for the rejected alternatives.
//! * Exactly one client (an AI agent, or `curl`) talks to it, over loopback, a few times a second
//!   at most. HTTP/1.1 with `Connection: close` and one thread per request is the entire
//!   protocol surface that needs to work.
//! * Zero new dependencies: the installer size is a product constraint (see D001), and a router
//!   that fits in 200 lines beats 2 MB of hyper.
//!
//! Security posture: loopback only, bearer token required on everything except `GET /` (the
//! self-describing document, which says nothing the caller did not already know by connecting).
//! The token lives in `<app config>/control-token` so it survives restarts — a token that rotates
//! would silently break every prompt the user has pasted into an assistant.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::engine::TrainRequest;
use crate::runtime::Runtime;
use crate::settings;

/// Hard limits. Without them a stuck client holds a thread forever and a malformed length hangs
/// the reader on a body that never arrives.
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// A running service. Cheap to clone; `stop` is the only way to end it, and it is asynchronous
/// (the accept loop notices within one poll interval).
#[derive(Clone)]
pub struct Handle {
    pub port: u16,
    pub token: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Handle {
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Bind and start serving. Falls back to an OS-assigned port when the requested one is taken —
/// the caller reports the real port in `SettingsView.control`, so "port busy" never becomes
/// "the feature silently does not work".
pub fn start(runtime: Runtime, requested_port: u16) -> Result<Handle, String> {
    let token = load_or_create_token(&runtime)?;

    let listener = TcpListener::bind(("127.0.0.1", requested_port))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(|error| format!("无法监听本地端口：{error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("无法读取监听地址：{error}"))?
        .port();

    let started = Instant::now();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Non-blocking accept so the loop can notice `stop`. A blocking `incoming()` would need a
    // wake-up connection from the stopper and a self-connect on shutdown, which is more moving
    // parts than a 120 ms poll for a service that answers a handful of requests a minute.
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("无法配置监听器：{error}"))?;

    let worker = runtime.clone();
    let worker_token = token.clone();
    let worker_stop = stop.clone();
    std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Accepted sockets inherit the listener's non-blocking flag on Windows; the
                    // per-request reader assumes blocking IO.
                    let _ = stream.set_nonblocking(false);
                    let runtime = worker.clone();
                    let token = worker_token.clone();
                    std::thread::spawn(move || handle(stream, &runtime, &token, started));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(120));
                }
                // A transient accept error must not kill the service; a closed listener would
                // spin here, so back off too.
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    });

    Ok(Handle { port, token, stop })
}

/// The token is generated once and then reused. Writing it next to `settings.json` also gives the
/// user a way to look it up without opening the app.
fn load_or_create_token(runtime: &Runtime) -> Result<String, String> {
    let path = runtime.config_dir().join("control-token");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let trimmed = text.trim();
        if trimmed.len() >= 16 {
            return Ok(trimmed.to_owned());
        }
    }
    let token = generate_token();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, &token).map_err(|error| format!("无法保存控制令牌：{error}"))?;
    Ok(token)
}

/// Not cryptographic randomness with a capital C — there is no CSPRNG in std and pulling `rand`
/// in for this is not worth a dependency. The input is nanoseconds + pid + a stack address, mixed
/// through splitmix64; the threat model is another user on the same machine guessing a 128-bit
/// string, and that is not reachable from 64 bits of weak entropy over a token that never leaves
/// the local disk.
fn generate_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let local = 0u8;
    let stack = &local as *const u8 as u64;
    let mut x = nanos ^ ((std::process::id() as u64) << 32) ^ stack.rotate_left(17);
    let mut token = String::with_capacity(32);
    for _ in 0..32 {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        token.push(std::char::from_digit((z % 16) as u32, 16).unwrap_or('0'));
    }
    token
}

/* ───────────────────────────── protocol ───────────────────────────── */

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Response {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: body.to_string(),
        }
    }

    fn text(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "text/markdown; charset=utf-8",
            body: body.into(),
        }
    }

    fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, json!({ "ok": false, "error": message.into() }))
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            500 => "Internal Server Error",
            _ => "Unknown",
        }
    }
}

fn handle(mut stream: TcpStream, runtime: &Runtime, token: &str, started: Instant) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let Ok(peer) = stream.peer_addr() else { return };
    // Belt and braces: the listener is already loopback-only, but a future change to bind
    // addresses must not silently expose this to the network.
    if !peer.ip().is_loopback() {
        let _ = write_response(&mut stream, &Response::error(401, "仅接受本机回环连接。"));
        return;
    }

    let Ok(reader_stream) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reader_stream);
    let Some(request) = read_request(&mut reader, &mut stream) else {
        return;
    };

    // `GET /` is the self-describing document and is readable without a token: an assistant needs
    // to be able to discover the shape of the API before it has been given the secret, and the
    // document contains no data — only the contract.
    let public = request.method == "GET" && (request.path == "/" || request.path == "/index.md");
    if !public && !authorised(&request, token) {
        let _ = write_response(
            &mut stream,
            &Response::json(
                401,
                json!({
                    "ok": false,
                    "error": "缺少或错误的令牌。请求头 Authorization: Bearer <token>，或查询参数 ?token=<token>。",
                    "hint": "令牌在应用的设置页「AI 操控」区块，也在配置目录的 control-token 文件里。"
                }),
            ),
        );
        return;
    }

    let response = route(&request, runtime, started);
    let _ = write_response(&mut stream, &response);
}

fn read_request(reader: &mut BufReader<TcpStream>, stream: &mut TcpStream) -> Option<Request> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.trim_end().split_whitespace();
    let method = parts.next()?.to_uppercase();
    let target = parts.next()?.to_owned();
    // Some HTTP/1.0 clients omit the version; anything present is ignored, because the only
    // version that matters here is "close after one response".
    let _ = parts.next();

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target.as_str(), ""),
    };
    let path = percent_decode(raw_path);
    let query = parse_query(raw_query);

    let mut headers = HashMap::new();
    let mut consumed = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        consumed += header.len();
        if consumed > MAX_HEADER_BYTES {
            return None;
        }
        let trimmed = header.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    // curl sends this for bodies over ~1 KB and then waits up to a second for permission. Saying
    // "go ahead" is one line and removes a second from every training request.
    if headers
        .get("expect")
        .map(|value| value.to_ascii_lowercase().contains("100-continue"))
        .unwrap_or(false)
    {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        let _ = stream.flush();
    }

    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
        .min(MAX_BODY_BYTES);
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return None;
    }

    Some(Request {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, response: &Response) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        response.status,
        response.reason(),
        response.content_type,
        response.body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(response.body.as_bytes())?;
    stream.flush()
}

fn authorised(request: &Request, token: &str) -> bool {
    let presented = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer ")))
        .map(str::to_owned)
        .or_else(|| request.query.get("token").cloned());
    match presented {
        Some(value) => constant_time_eq(value.as_bytes(), token.as_bytes()),
        None => false,
    }
}

/// Length-independent comparison. The timing channel is not the real threat here, but it costs
/// six lines and stops the habit from spreading to a context where it would matter.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn parse_query(raw: &str) -> HashMap<String, String> {
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(&value.replace('+', " "))),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// Percent-decoding into a `String` with UTF-8 awareness: dataset paths are Windows paths with
/// Chinese folder names, and a byte-wise decode would mangle them into invalid UTF-8.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &input[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/* ───────────────────────────── routes ───────────────────────────── */

fn route(request: &Request, runtime: &Runtime, started: Instant) -> Response {
    let path = request.path.trim_end_matches('/').to_owned();
    let path = if path.is_empty() { "/".to_string() } else { path };

    match (request.method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.md") => Response::text(guide(runtime)),
        ("GET", "/health") => Response::json(
            200,
            json!({
                "ok": true,
                "app": "yolo-trainer-lab",
                "version": env!("CARGO_PKG_VERSION"),
                "uptime_s": started.elapsed().as_secs(),
                "running": runtime.engine().is_running(),
                "engine": runtime.view().engine_resolved,
                "platform": std::env::consts::OS,
            }),
        ),
        ("GET", "/state") => {
            let snapshot = runtime.run_snapshot();
            let since = request
                .query
                .get("since")
                .and_then(|value| value.parse::<u64>().ok());
            match since {
                Some(cursor) => {
                    let (next, events) = runtime.events_since(cursor);
                    Response::json(
                        200,
                        json!({ "ok": true, "state": snapshot, "cursor": next, "events": events }),
                    )
                }
                None => Response::json(200, snapshot),
            }
        }
        ("GET", "/events") => {
            let since = request
                .query
                .get("since")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let (cursor, events) = runtime.events_since(since);
            Response::json(200, json!({ "ok": true, "cursor": cursor, "events": events }))
        }
        ("POST", "/train") => start_training(request, runtime),
        ("POST", "/predict") => run_prediction(request, runtime),
        ("POST", "/stop") => match runtime.stop_training() {
            Ok(()) => Response::json(200, json!({ "ok": true, "stopped": true })),
            Err(error) => Response::error(500, error),
        },
        ("GET", "/datasets/scan") => {
            let Some(target) = request.query.get("path") else {
                return Response::error(400, "缺少查询参数 path=/数据集/目录");
            };
            if target.trim().is_empty() {
                return Response::error(400, "path 不能为空");
            }
            match crate::dataset::scan(target) {
                Ok(summary) => Response::json(200, json!({ "ok": true, "dataset": summary })),
                Err(error) => Response::error(400, error),
            }
        }
        ("GET", "/settings") => {
            Response::json(200, json!({ "ok": true, "settings": runtime.view() }))
        }
        ("PUT", "/settings") => {
            let parsed: settings::Settings = match serde_json::from_slice(&request.body) {
                Ok(value) => value,
                Err(error) => return Response::error(400, format!("JSON 解析失败：{error}")),
            };
            match runtime.save_settings(parsed) {
                Ok(view) => Response::json(200, json!({ "ok": true, "settings": view })),
                Err(error) => Response::error(500, error),
            }
        }
        (method, path) if path == "/train" || path == "/stop" || path == "/settings" => {
            Response::error(405, format!("{method} 不被支持，该路径只接受特定的方法。"))
        }
        (method, path) if path == "/predict" => {
            Response::error(405, format!("{method} 不被支持，请用 POST /predict。"))
        }
        _ => Response::error(
            404,
            "没有这个端点。GET / 查看完整的使用说明。",
        ),
    }
}

fn start_training(request: &Request, runtime: &Runtime) -> Response {
    if runtime.engine().is_running() {
        return Response::error(409, "已有训练在运行。先 POST /stop 或等它结束。");
    }

    // Accepted body shapes, in order of convenience for an assistant writing JSON by hand:
    //   1. {"dataset": "...", "epochs": 10}                    — everything else defaulted
    //   2. {"architecture": "...", "dataset": "...", ...}      — the full TrainRequest
    let partial: Value = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => return Response::error(400, format!("JSON 解析失败：{error}")),
    };
    let dataset = partial
        .get("dataset")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if dataset.is_empty() {
        return Response::error(
            400,
            "缺少 dataset。它是一个包含 dataset.yaml 的目录，或 yaml 文件本身的绝对路径。",
        );
    }
    if !Path::new(&dataset).exists() {
        return Response::error(400, format!("dataset 路径不存在：{dataset}"));
    }

    let defaults = runtime.settings();
    let train = TrainRequest {
        architecture: partial
            .get("architecture")
            .and_then(Value::as_str)
            .unwrap_or("yolo26n")
            .to_owned(),
        dataset,
        epochs: uint(&partial, "epochs").unwrap_or(1),
        batch: uint(&partial, "batch").unwrap_or(8),
        imgsz: uint(&partial, "imgsz"),
        device: partial
            .get("device")
            .and_then(Value::as_str)
            .unwrap_or(&defaults.default_device)
            .to_owned(),
    };

    let accepted = json!({
        "ok": true,
        "accepted": {
            "architecture": train.architecture,
            "epochs": train.epochs,
            "batch": train.batch,
            "imgsz": train.imgsz,
            "device": train.device,
        },
        "note": "已开始。轮询 GET /state 或 GET /events?since=<cursor> 获取进度。"
    });

    match runtime.start_training(&train) {
        Ok(()) => Response::json(202, accepted),
        Err(error) => Response::error(500, error),
    }
}

/// Run one image through a trained model.
///
/// `model` is optional so the common case is one field: "score the best model from the last run".
/// A request that supplies neither is a client mistake worth naming, not a 500.
fn run_prediction(request: &Request, runtime: &Runtime) -> Response {
    let parsed: crate::predict::PredictRequest = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => return Response::error(400, format!("JSON 解析失败：{error}")),
    };
    if parsed.source.trim().is_empty() {
        return Response::error(400, "缺少 source：要推理的图片绝对路径。");
    }

    let model = match parsed
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(path) => PathBuf::from(path),
        None => {
            let snapshot = runtime.run_snapshot();
            match snapshot.get("best").and_then(Value::as_str) {
                Some(best) => PathBuf::from(best),
                None => {
                    return Response::error(
                        400,
                        "还没有可用的模型：先 POST /train 跑一次，或在 body 里给 model=<best.bpk 的路径>。",
                    )
                }
            }
        }
    };
    if !model.is_file() {
        return Response::error(400, format!("模型文件不存在：{}", model.display()));
    }

    let engine = match crate::engine::resolve(runtime.settings().engine_path.as_deref()) {
        Ok(path) => path,
        Err(error) => return Response::error(500, error),
    };

    // Never let the engine write next to the source image: that would drop `*-detections.png`
    // into a dataset directory and the next scan would count it as training data.
    let default_dir = runtime.output_root().join("predictions");
    match crate::predict::run(&engine, &parsed, &model, &default_dir) {
        Ok(result) => Response::json(200, json!({ "ok": true, "result": result })),
        Err(error) => Response::error(400, error),
    }
}

fn uint(value: &Value, key: &str) -> Option<u32> {    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

/* ───────────────────────────── the document ───────────────────────────── */

/// `GET /` — written for an assistant to read once and then act on. Everything an agent needs is
/// here; nothing it needs to guess is.
fn guide(runtime: &Runtime) -> String {
    let view = runtime.view();
    let port = view.control.port;
    let base = format!("http://127.0.0.1:{port}");
    format!(
        r#"# YOLO Trainer · 本地控制接口

你在操控一台 Windows 机器上的 YOLO 目标检测训练器。它已经装好、引擎可用、不需要你安装任何东西。

- 基地址：`{base}`
- 鉴权：除本文件外，所有请求都要带请求头 `Authorization: Bearer <token>`
  （也可以用查询参数 `?token=<token>`）。令牌由用户提供；如果没给，让用户去应用的
  「设置 → AI 操控」复制。
- 响应一律是 JSON（本文件除外），字段名用 snake_case。
- 所有路径都是 **Windows 绝对路径**，反斜杠要写成 `\\`，或者用 `/`。

## 端点

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| GET | `/` | 本文件 |
| GET | `/health` | 服务与引擎是否活着 |
| GET | `/state` | 当前（或最近一次）训练的快照 |
| GET | `/state?since=N` | 快照 + 序号大于 N 的新事件 |
| GET | `/events?since=N` | 只要新事件，不重复快照 |
| POST | `/train` | 开始训练 |
| POST | `/stop` | 中止训练 |
| POST | `/predict` | 用训练好的模型给一张图推理 |
| GET | `/datasets/scan?path=...` | 检查一个数据集目录是否可用 |
| GET | `/settings` | 读配置 |
| PUT | `/settings` | 改配置（整体替换，缺字段用默认值） |

## 典型流程

1. 先扫描数据集，确认类别和图片数，别拿坏数据集去训练：

```bash
curl -s -H "Authorization: Bearer $TOKEN" \
  "{base}/datasets/scan?path=E:/data/coco-mini"
```

返回 `dataset.train_images`、`dataset.classes`、`dataset.missing_labels`、`dataset.warning`。
`missing_labels > 0` 说明有图片没标注，可以训练但效果会打折；`train_images == 0` 就别开始。
`dataset.format` 是 `yolo`（检测/分割）或 `imagenet-folders`（分类），任务与格式必须匹配：
分类只能用 `imagenet-folders`，检测和分割只能用 `yolo`。

2. 开始训练（`epochs` 是轮数，`batch` 是批大小，`device` 是 `auto|cpu|gpu`）：

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{{"dataset":"E:/data/coco-mini","epochs":20,"batch":4,"device":"auto"}}' \
  "{base}/train"
```

成功返回 `202` 和实际接受的参数。已经有训练在跑时返回 `409`。

3. 轮询进度。第一次拿到 `cursor`，之后每次把它带回去，只取增量：

```bash
curl -s -H "Authorization: Bearer $TOKEN" "{base}/state?since=0"
curl -s -H "Authorization: Bearer $TOKEN" "{base}/state?since=<上一步的 cursor>"
```

`state` 的关键字段：

| 字段 | 含义 |
| --- | --- |
| `status` | `idle` / `running` / `done` / `failed` |
| `epoch` / `total_epochs` | 进度 |
| `last_loss` | 本轮平均损失，越低越好 |
| `backend` | 实际用的后端：`cpu` 或 `gpu`（`auto` 可能回退到 `cpu`） |
| `best` | 训练成功后导出的权重路径（`.bpk`），用于推理 |
| `error` | 失败时为 `{{code, message, hint}}` |

4. 结束。`status` 变成 `done` 就停，`best` 是产物；变成 `failed` 就把 `error.hint` 讲给用户。

5. 验证产物：拿一张图跑推理，确认模型真的学到了东西（而不是只看 loss 在降）。

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{{"source":"E:/data/coco-mini/images/val/0001.jpg"}}' \
  "{base}/predict"
```

`model` 可以省略，默认用最近一次训练导出的 `best`。返回 `result.prediction` 就是引擎的原始
JSON：检测模型给 `detections`（`xmin/ymin/xmax/ymax/confidence/class_name`），分割模型另给
`masks`，分类模型给 `classes`（top-5）。`result.output` 是标注后的图片路径，分类模型没有这个
字段——它不画图。

推理固定跑在 CPU 上（`device` 可传 `cpu`/`gpu`）。标注图会写进应用自己的目录，
**不会**写到你图片旁边。

## 事件流（`/events`）

每个事件有 `type`：

- `start` — `run`（输出目录）、`backend`、`adapter`、`epochs`、`batch_size`、`class_names`
- `epoch` — `epoch`、`epochs`、`loss`、`fitness`、`elapsed_s`
- `end` — `best`、`last`
- `error` — `code`、`message`、`hint`
- `exit` — `code`（进程退出码；`101` 多半是显卡驱动崩了）
- `log` — `line`（引擎的人话日志，排查时看这个）

## 错误码（`error.code`）

| code | 意思 | 该怎么办 |
| --- | --- | --- |
| `GPU_FALLBACK` | 显卡探测失败，已自动用 CPU | 只是慢，不用管 |
| `GPU_LOST` | 训练中显卡掉了 | 建议用户切 `"device":"cpu"` 重试 |
| `NO_DATASET` | `dataset.yaml` 打不开 | 让用户确认路径 |
| `BAD_ANNOTATION` | 标注格式错 | 指出格式，别自己改文件 |
| `EMPTY_DATASET` | 训练集没图片 | 换数据集 |
| `CLASS_MISMATCH` | 续训时类别不一致 | 换回原类别 |
| `OOM` | 显存/内存不够 | 把 `batch` 或 `imgsz` 调小重试 |
| `INTERNAL` | 其它 | 展示 `message` |

## 改配置

```bash
curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{{"default_device":"cpu"}}' "{base}/settings"
```

可写字段：`engine_path`（null = 自动检测）、`default_device`、`output_dir`（null = 默认）、
`control_enabled`、`control_port`（改端口需要重启应用）。

## 注意事项

- 如果这台机器开着 HTTP 代理（`http_proxy` / `HTTPS_PROXY` 环境变量），curl 会把
  `127.0.0.1` 的请求也发给代理，结果是 `502` 或"连接被拒绝"，看起来像服务没启动。
  加 `--noproxy 127.0.0.1`，或设 `NO_PROXY=127.0.0.1`。浏览器和大多数 API 客户端会自动
  绕过回环地址，但 curl 不会。
- 一次只能跑一个训练。要换参数必须等它结束或先 `POST /stop`。
- 训练把产物写到 `{output}`，每轮新建一个子目录。
- 不要直接改 `settings.json` 或删 `runs/`，通过接口或用应用的界面操作。
- 这台机器当前引擎：`{engine}`
"#,
        base = base,
        output = view.output_dir,
        engine = view
            .engine_resolved
            .clone()
            .unwrap_or_else(|| "未找到（用户需要在设置里指定）".to_string()),
    )
}
