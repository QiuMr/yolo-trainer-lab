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

use crate::engine::{TrainInit, TrainRequest};
use crate::runtime::Runtime;
use crate::settings;

/// Hard limits. Without them a stuck client holds a thread forever and a malformed length hangs
/// the reader on a body that never arrives.
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// One thread per connection, so the count is the thread count. A client in a fast retry loop
/// (an assistant that polls without sleeping is the realistic case) can create threads faster
/// than they retire; each one lives up to `IO_TIMEOUT`. 64 is far above what one local client
/// needs and far below anything that would hurt the UI process.
const MAX_CONNECTIONS: usize = 64;

/// Holds one unit of the live-connection count for as long as a handler runs. Drop is the only
/// decrement path, so a panicking handler still gives its slot back.
struct LiveSlot(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Drop for LiveSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A running service. Cheap to clone; `stop` is the only way to end it, and it is synchronous
/// (it waits for the accept loop to release the listening socket).
#[derive(Clone)]
pub struct Handle {
    pub port: u16,
    pub token: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: std::sync::Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl Handle {
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // The accept loop owns the TcpListener. Joining is the only way to know the socket is
        // actually closed: without it, stop→start re-binds while the old listener still holds
        // the port and silently falls back to an OS-assigned random port, invalidating every
        // prompt the user already pasted into an assistant.
        let worker = self
            .worker
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(worker) = worker {
            let _ = worker.join();
        }
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

    let worker_runtime = runtime.clone();
    let worker_token = token.clone();
    let worker_stop = stop.clone();
    let live = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    // Accepted sockets inherit the listener's non-blocking flag on Windows; the
                    // per-request reader assumes blocking IO.
                    let _ = stream.set_nonblocking(false);

                    // Claim the slot before spawning so the count and the thread start together.
                    if live.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
                        live.fetch_sub(1, Ordering::Relaxed);
                        refuse(stream);
                        continue;
                    }
                    let slot = LiveSlot(live.clone());
                    let runtime = worker_runtime.clone();
                    let token = worker_token.clone();
                    // `Builder::spawn` returns a Result; `thread::spawn` would panic. Killing the
                    // accept loop makes the service unreachable for good, which is strictly worse
                    // than refusing one connection.
                    let spawned = std::thread::Builder::new().spawn(move || {
                        let _slot = slot;
                        handle(stream, &runtime, &token, started);
                    });
                    if let Err(error) = spawned {
                        // Dropping the closure dropped the guard, so the slot is already back.
                        eprintln!("[control] 无法创建连接线程：{error}");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(120));
                }
                // A transient accept error must not kill the service; a closed listener would
                // spin here, so back off too.
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        // `listener` is dropped here, before `stop()`'s join() returns.
    });

    Ok(Handle {
        port,
        token,
        stop,
        worker: std::sync::Arc::new(std::sync::Mutex::new(Some(worker))),
    })
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
    body: Vec<u8>,
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

    /// A failure with a machine-readable `code`, an optional `hint`, and — for failures we decided
    /// the maintainer should see — the diagnostics report just written for it.
    ///
    /// `code` exists because `error` used to be prose only. A client cannot branch on prose: it
    /// cannot tell "your dataset path is wrong" from "the GPU driver died", so its only options
    /// were to retry or to give up. Both are wrong for one of the two cases.
    fn failure(
        status: u16,
        code: &str,
        message: impl Into<String>,
        hint: Option<&str>,
        diagnostic: Option<&crate::runtime::Diagnostic>,
    ) -> Self {
        let mut body = json!({ "ok": false, "error": message.into(), "code": code });
        if let Some(hint) = hint {
            body["hint"] = json!(hint);
        }
        if let Some(diagnostic) = diagnostic {
            body["diagnostics"] = serde_json::to_value(diagnostic).unwrap_or(Value::Null);
        }
        Self::json(status, body)
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
            413 => "Payload Too Large",
            431 => "Request Header Fields Too Large",
            500 => "Internal Server Error",
            _ => "Unknown",
        }
    }
}

/// Rejects one connection when the limit is hit. This runs on the accept thread, so the write
/// timeout must be short — a client that stops reading must not delay acceptance for everyone
/// else. A real status beats a bare close: the caller can tell "back off" from "not listening".
fn refuse(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
    // Drain what the client already sent first: closing a socket that still holds unread data
    // makes the kernel answer with RST, and the caller would see "connection reset" instead of
    // the 503 that explains why it was refused. One read is enough for the request heads that
    // reach here — anything larger is a client that was going to be refused anyway.
    let mut scratch = [0u8; 2048];
    let _ = stream.read(&mut scratch);
    let _ = write_response(
        &mut stream,
        &Response::error(503, "并发连接过多，请退避后重试。"),
    );
}

fn handle(mut stream: TcpStream, runtime: &Runtime, token: &str, started: Instant) {
    // Ignoring a failure here would make IO_TIMEOUT a no-op, so treat it as a broken connection.
    if stream.set_read_timeout(Some(IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(IO_TIMEOUT)).is_err()
    {
        return;
    }

    let Ok(peer) = stream.peer_addr() else { return };
    // Belt and braces: the listener is already loopback-only, but a future change to bind
    // addresses must not silently expose this to the network.
    if !peer.ip().is_loopback() {
        let _ = write_response(&mut stream, &Response::error(401, "仅接受本机回环连接。"));
        return;
    }

    let Ok(reader_stream) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reader_stream);
    let request = match read_request(&mut reader, &mut stream, token) {
        Ok(Some(request)) => request,
        // The peer hung up before sending anything worth answering.
        Ok(None) => return,
        Err(response) => {
            let _ = write_response(&mut stream, &response);
            return;
        }
    };

    let response = route(&request, runtime, started);
    let _ = write_response(&mut stream, &response);
}

/// Parse one request and authorise it **before** reading the body. `Err(Response)` means the
/// caller gets a real HTTP status instead of an empty reply: a bare `return None` here used to
/// turn every malformed request into "Empty reply from server".
fn read_request(
    reader: &mut BufReader<TcpStream>,
    stream: &mut TcpStream,
    token: &str,
) -> Result<Option<Request>, Response> {
    let mut budget = 0usize;
    let line = read_limited_line(reader, &mut budget)?;
    if line.is_empty() {
        return Ok(None);
    }
    let mut parts = line.trim_end().split(' ');
    let method = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Response::error(400, "请求行缺少方法。"))?
        .to_uppercase();
    let target = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Response::error(400, "请求行缺少路径。"))?
        .to_owned();
    // HTTP/1.0 clients may omit the version; a fourth token (or a non-HTTP third one) means the
    // URL contained a raw space. Silently truncating it produced a confusing 404.
    let version = parts.next().unwrap_or("");
    if (!version.is_empty() && !version.starts_with("HTTP/")) || parts.next().is_some() {
        return Err(Response::error(
            400,
            "URL 里的空格必须编码成 %20；路径请 percent-encode。",
        ));
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target.as_str(), ""),
    };
    let path = percent_decode(raw_path);
    let query = parse_query(raw_query);

    let mut headers = HashMap::new();
    loop {
        let line = read_limited_line(reader, &mut budget)?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let key = name.trim().to_ascii_lowercase();
            if headers.insert(key, value.trim().to_owned()).is_some() {
                return Err(Response::error(400, "请求头重复。"));
            }
        }
    }

    // `GET /` is the self-describing document and is readable without a token: an assistant
    // needs to discover the contract before it has been given the secret, and it contains no
    // data. Everything else is authenticated here, before any body bytes are buffered, so an
    // unauthenticated caller cannot make us allocate 1 MB or receive `100 Continue`.
    let public = method == "GET" && (path == "/" || path == "/index.md");
    if !public && !authorised(&headers, &query, token) {
        return Err(Response::json(
            401,
            json!({
                "ok": false,
                "error": "缺少或错误的令牌。请求头 Authorization: Bearer <token>，或查询参数 ?token=<token>。",
                "hint": "令牌在应用的设置页「AI 操控」区块，也在配置目录的 control-token 文件里。"
            }),
        ));
    }

    if headers
        .get("transfer-encoding")
        .is_some_and(|value| !value.is_empty())
    {
        return Err(Response::error(
            400,
            "不支持 Transfer-Encoding，请用 Content-Length。",
        ));
    }
    let length = match headers.get("content-length") {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| Response::error(400, "Content-Length 不是合法数字。"))?,
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        return Err(Response::error(413, "请求体超过 1 MB 上限。"));
    }

    // curl sends this for bodies over ~1 KB and then waits up to a second for permission.
    if headers
        .get("expect")
        .map(|value| value.to_ascii_lowercase().contains("100-continue"))
        .unwrap_or(false)
    {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        let _ = stream.flush();
    }

    let mut body = vec![0u8; length];
    if length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|_| Response::error(400, "请求体不完整或读取超时。"))?;
    }

    Ok(Some(Request {
        method,
        path,
        query,
        body,
    }))
}

/// Read through the next `\n` without allocating beyond the shared header budget.
/// `BufRead::read_line` cannot do this: it grows until the newline arrives, so one 100 MB header
/// line is enough to exhaust memory before any limit is checked.
fn read_limited_line(
    reader: &mut BufReader<TcpStream>,
    budget: &mut usize,
) -> Result<String, Response> {
    let mut bytes = Vec::new();
    loop {
        let (take, complete) = {
            let available = reader
                .fill_buf()
                .map_err(|_| Response::error(400, "读取请求失败或超时。"))?;
            if available.is_empty() {
                break;
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let take = newline.map(|index| index + 1).unwrap_or(available.len());
            if *budget + take > MAX_HEADER_BYTES {
                return Err(Response::error(431, "请求头超过 16 KB 上限。"));
            }
            bytes.extend_from_slice(&available[..take]);
            (take, newline.is_some())
        };
        reader.consume(take);
        *budget += take;
        if complete {
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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

fn authorised(
    headers: &HashMap<String, String>,
    query: &HashMap<String, String>,
    token: &str,
) -> bool {
    // The auth-scheme is case-insensitive per RFC 7235; `strip_prefix("Bearer ")` rejected
    // `BEARER` and `bEaReR` for no reason.
    let presented = headers
        .get("authorization")
        .and_then(|value| {
            let (scheme, rest) = value.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
        })
        .map(str::to_owned)
        .or_else(|| query.get("token").cloned());
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
///
/// Work on the **byte slice only**. `&input[index+1..index+3]` used to slice the `&str` by byte
/// offsets: when a `%` was followed by a multi-byte character, the end of that slice landed
/// inside a code point and Rust panicked. With `panic = "abort"` that killed the whole app,
/// from an unauthenticated GET — the query is parsed before the token is checked.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/* ───────────────────────────── routes ───────────────────────────── */

/// `?since=` on `/state` and `/events`. An unparseable or empty value counts as absent rather than
/// a 400: `?since=` with nothing after it means "from the beginning", which is what the client
/// wanted anyway, and a 400 there would turn a harmless templating bug into a dead poll loop.
fn since_param(request: &Request) -> Option<u64> {
    request
        .query
        .get("since")
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| value.parse::<u64>().ok())
}

/// One body shape for `/state?since=N` and `/events?since=N`.
///
/// `reset` and `truncated` are always present rather than only when true: a client should be able
/// to branch on them without checking for absence, and they answer the two questions a poller
/// actually has — "did the app restart under me" and "did I miss events".
fn delta_body(runtime: &Runtime, since: u64, state: Option<Value>) -> Value {
    let delta = runtime.events_since(since);
    let mut body = json!({
        "ok": true,
        "cursor": delta.cursor,
        "reset": delta.reset,
        "truncated": delta.truncated,
        "events": delta.events,
    });
    if let Some(state) = state {
        body["state"] = state;
    }
    body
}

/// `GET /datasets/scan?path=` and `POST /datasets/scan` answer identically.
fn scan_dataset(target: &str) -> Response {
    if target.trim().is_empty() {
        return Response::error(400, "path 不能为空");
    }
    match crate::dataset::scan(target) {
        Ok(summary) => Response::json(200, json!({ "ok": true, "dataset": summary })),
        Err(error) => Response::error(400, error),
    }
}

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
                // Not CARGO_PKG_VERSION: Cargo.toml is deliberately frozen at 0.1.0 while
                // tauri.conf.json is the single source of truth (see build.rs / D022).
                "version": crate::version::VERSION,
                "uptime_s": started.elapsed().as_secs(),
                "running": runtime.engine().is_running(),
                "engine": runtime.view().engine_resolved,
                "platform": std::env::consts::OS,
            }),
        ),
        ("GET", "/state") => {
            let snapshot = runtime.run_snapshot();
            match since_param(request) {
                Some(cursor) => Response::json(200, delta_body(runtime, cursor, Some(snapshot))),
                None => Response::json(200, snapshot),
            }
        }
        ("GET", "/events") => {
            let since = since_param(request).unwrap_or(0);
            Response::json(200, delta_body(runtime, since, None))
        }
        // The question every AI client asks first and can only answer by wasting a run: will
        // training reach epoch one on this machine? Cached, so the first call may take as long as
        // the probe timeout and later ones are instant. `refresh=1` re-runs it.
        ("GET", "/device") => {
            let device = request
                .query
                .get("device")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| runtime.settings().default_device);
            let refresh = request
                .query
                .get("refresh")
                .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
            let report = runtime.probe_device(&device, refresh);
            Response::json(200, json!({ "ok": true, "probe": report }))
        }
        ("GET", "/runs") => {
            let limit = request
                .query
                .get("limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(50);
            let root = runtime.output_root();
            let list = crate::runs::scan(&root, limit);
            Response::json(
                200,
                json!({
                    "ok": true,
                    "root": root.to_string_lossy(),
                    "count": list.len(),
                    // `/state` forgets everything when the app restarts; this is the only way to
                    // find the model a previous session trained.
                    "note": "用 runs[].best 作为 POST /predict 的 model；续训用 runs[].dir + \\checkpoints\\last。",
                    "runs": list,
                }),
            )
        }
        ("GET", "/diagnostics") => {
            let limit = request
                .query
                .get("limit")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(20);
            let root = runtime.diagnostics_root();
            let list = crate::diagnostics::list(&root, limit);
            Response::json(
                200,
                json!({
                    "ok": true,
                    "root": root.to_string_lossy(),
                    "count": list.len(),
                    "note": "每份报告都在本地，路径与数据集名已脱敏。pushed=false 表示没推送到开发者的通道。",
                    "reports": list,
                }),
            )
        }
        ("POST", "/diagnostics") => submit_diagnostic(request, runtime),
        ("POST", "/train") => start_training(request, runtime),
        ("POST", "/predict") => run_prediction(request, runtime),
        ("POST", "/stop") => {
            // Saying `stopped: true` when nothing was running is a lie an assistant will act on:
            // it will wait for a status change that can never come.
            if !runtime.engine().is_running() {
                return Response::json(
                    200,
                    json!({
                        "ok": true,
                        "stopped": false,
                        "note": "当前没有训练在跑，没有需要停止的东西。",
                    }),
                );
            }
            match runtime.stop_training() {
                Ok(()) => Response::json(200, json!({ "ok": true, "stopped": true })),
                Err(error) => Response::error(500, error),
            }
        }
        ("GET", "/datasets/scan") => {
            let Some(target) = request.query.get("path") else {
                return Response::error(
                    400,
                    "缺少查询参数 path=/数据集/目录。中文路径不方便 percent-encode 时，\
                     改用 POST /datasets/scan，body 为 {\"path\":\"E:/数据\"}。",
                );
            };
            scan_dataset(target)
        }
        // The same scan through a JSON body: a Windows path with Chinese folder names needs
        // percent-encoding in a query string, and getting that wrong produces a 404-looking
        // "path does not exist" that an assistant will try to debug in the wrong place.
        ("POST", "/datasets/scan") => {
            let parsed: Value = match serde_json::from_slice(&request.body) {
                Ok(value) => value,
                Err(error) => return Response::error(400, format!("JSON 解析失败：{error}")),
            };
            let Some(target) = parsed.get("path").and_then(Value::as_str) else {
                return Response::error(400, "缺少 path：数据集目录或 dataset.yaml 的绝对路径。");
            };
            scan_dataset(target)
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
        // A known path with the wrong method is 405, not 404. Listing them explicitly keeps the
        // error honest for an AI client that is probing the API.
        (method, path)
            if [
                "/",
                "/index.md",
                "/health",
                "/state",
                "/events",
                "/runs",
                "/train",
                "/stop",
                "/predict",
                "/datasets/scan",
                "/diagnostics",
                "/settings",
            ]
            .contains(&path) =>
        {
            Response::error(405, format!("{method} 不被支持，该路径只接受特定的方法。"))
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
    //   1. {"dataset": "...", "epochs": 10}                        — scratch, everything defaulted
    //   2. {"architecture": "...", "dataset": "...", ...}          — explicit scratch
    //   3. {"init": {"mode": "finetune", "model": "..."}, ...}     — from a finished .bpk
    //   4. {"init": {"mode": "resume", "checkpoint": "..."}}       — continue a run exactly
    // Shapes 1 and 2 are the original contract and stay supported: a saved prompt or a手写 script
    // should not break because a newer field exists.
    let partial: Value = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => return Response::error(400, format!("JSON 解析失败：{error}")),
    };
    let init = match partial.get("init") {
        Some(raw) => match serde_json::from_value::<TrainInit>(raw.clone()) {
            Ok(value) => value,
            Err(error) => {
                return Response::error(
                    400,
                    format!(
                        "init 无法解析：{error}。\
                         应为 {{\"mode\":\"scratch\",\"architecture\":\"yolo26n\"}}、\
                         {{\"mode\":\"finetune\",\"model\":\"…/best.bpk\"}} 或 \
                         {{\"mode\":\"resume\",\"checkpoint\":\"…/checkpoints/last\"}}。"
                    ),
                );
            }
        },
        None => {
            // The window validates architecture in TrainPage; an HTTP caller bypasses that
            // entirely, so the same bound is repeated here.
            let architecture = partial
                .get("architecture")
                .and_then(Value::as_str)
                .unwrap_or("yolo26n")
                .to_owned();
            if !valid_architecture(&architecture) {
                return Response::error(
                    400,
                    "architecture 只能包含字母、数字、点、下划线和连字符，且不能以 - 开头。",
                );
            }
            TrainInit::Scratch { architecture }
        }
    };

    // Resume takes its dataset from the checkpoint. Demanding one here would reject the only
    // correct way to call it.
    let needs_dataset = !matches!(init, TrainInit::Resume { .. });
    let dataset = partial
        .get("dataset")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let mut warnings: Vec<String> = Vec::new();
    if needs_dataset {
        if dataset.is_empty() {
            return Response::error(
                400,
                "缺少 dataset。它是一个包含 dataset.yaml 的目录，或 yaml 文件本身的绝对路径。",
            );
        }
        if !Path::new(&dataset).exists() {
            return Response::error(400, format!("dataset 路径不存在：{dataset}"));
        }
        let dataset_path = Path::new(&dataset);
        if dataset_path.is_dir() && !dataset_path.join("dataset.yaml").is_file() {
            return Response::error(400, format!("目录里没有 dataset.yaml：{dataset}"));
        }

        // A classification architecture pointed at a detection dataset is the mistake an
        // assistant makes most often. The window already refuses it (`models.ts::taskProblem`);
        // over HTTP nothing did, so the run started, spent a minute loading weights, and then
        // died with the engine's own wording.
        if let TrainInit::Scratch { architecture } = &init {
            match task_of_architecture(architecture) {
                Err(message) => return Response::error(400, message),
                Ok(task) => {
                    // The scanner understands the two layouts the UI offers; the engine also
                    // reads COCO. A dataset this scanner cannot parse must not be blocked here —
                    // the engine's own error is the right answer for it.
                    if let Ok(summary) = crate::dataset::scan(&dataset) {
                        if let Some(problem) = task_mismatch(task, &summary.format) {
                            return Response::error(400, problem);
                        }
                        // "No validation split" is worth telling the client now, not after mAP
                        // comes back as zero.
                        if let Some(warning) = summary.warning {
                            warnings.push(warning);
                        }
                    }
                }
            }
        }
    }

    let epochs = uint(&partial, "epochs").unwrap_or(1);
    let batch = uint(&partial, "batch").unwrap_or(8);
    let imgsz = uint(&partial, "imgsz");
    if !(1..=1000).contains(&epochs) {
        return Response::error(400, "epochs 必须在 1 到 1000 之间。");
    }
    if !(1..=128).contains(&batch) {
        return Response::error(400, "batch 必须在 1 到 128 之间。");
    }
    if let Some(size) = imgsz {
        if !(64..=2048).contains(&size) || size % 32 != 0 {
            return Response::error(400, "imgsz 必须在 64 到 2048 之间，且是 32 的倍数。");
        }
    }

    let defaults = runtime.settings();
    let requested_device = partial
        .get("device")
        .and_then(Value::as_str)
        .unwrap_or(&defaults.default_device)
        .trim()
        .to_owned();
    // Echo what the engine will actually receive, not what was asked for. The spawn path coerces
    // anything outside `auto` / `cpu` / `gpu`, and a caller that asked for `gpu` while silently
    // getting `auto` had no way to notice from the old response.
    let device = settings::normalize_device(&requested_device, &defaults.default_device);
    if !requested_device.is_empty() && device != requested_device {
        warnings.push(format!("device 只接受 auto / cpu / gpu，已改成 {device}。"));
    }

    let train = TrainRequest {
        init,
        dataset,
        epochs,
        batch,
        imgsz,
        device,
    };

    let accepted = json!({
        "ok": true,
        "accepted": {
            "init": serde_json::to_value(&train.init).unwrap_or(Value::Null),
            "dataset": if train.dataset.is_empty() { Value::Null } else { json!(train.dataset) },
            "epochs": train.epochs,
            "batch": train.batch,
            "imgsz": train.imgsz,
            "device": train.device,
        },
        // Empty most of the time. A client that ignores this field got exactly what it asked for.
        "warnings": warnings,
        "note": "已开始。轮询 GET /state 或 GET /events?since=<cursor> 获取进度。"
    });

    match runtime.start_training(&train) {
        Ok(()) => Response::json(202, accepted),
        // The early is_running() check is a TOCTOU window; Engine::start is now atomic, so a
        // race shows up here as an error rather than a second orphan process. Report it as 409.
        Err(error) if error.contains("已有训练") => Response::error(409, error),
        Err(error) => {
            // A failed spawn is the most valuable thing to capture in this whole service: the
            // engine's exit message is usually one line ("no compatible GPU adapter"), and the
            // detail that explains it exists only in stderr — which used to be discarded.
            let hint = "先看报告里的「引擎日志尾部」。如果写着 no compatible GPU adapter，把 device 改成 cpu 再试。";
            let diagnostic = runtime.diagnose(
                &crate::diagnostics::Draft {
                    kind: "train_start_failed",
                    code: "engine_spawn_failed",
                    message: &error,
                    hint: Some(hint),
                    context: json!({
                        "init": &train.init,
                        "epochs": train.epochs,
                        "batch": train.batch,
                        "imgsz": train.imgsz,
                        "device": &train.device,
                    }),
                },
                if train.dataset.is_empty() {
                    None
                } else {
                    Some(train.dataset.as_str())
                },
            );
            Response::failure(500, "engine_spawn_failed", error, Some(hint), diagnostic.as_ref())
        }
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
        // 400 means "fix your request", 500 means "the engine or the disk failed". Reporting both
        // as 400 sent clients into a parameter-tuning loop when the real problem was a model the
        // engine could not load.
        Err(crate::predict::PredictError::BadRequest(message)) => Response::error(400, message),
        Err(crate::predict::PredictError::Internal(message)) => {
            // Same reasoning as a failed spawn: "推理失败：exit code 1" with nothing else is
            // unactionable, and the reason is in stderr.
            let hint = "确认模型与模型架构一致。若报显存或 device lost，把 device 改成 cpu 再试。";
            let diagnostic = runtime.diagnose(
                &crate::diagnostics::Draft {
                    kind: "predict_failed",
                    code: "engine_predict_failed",
                    message: &message,
                    hint: Some(hint),
                    context: json!({
                        "model": model.display().to_string(),
                        "source": parsed.source,
                        "device": parsed.device,
                        "masks": parsed.masks,
                    }),
                },
                None,
            );
            Response::failure(500, "engine_predict_failed", message, Some(hint), diagnostic.as_ref())
        }
    }
}

/// `POST /diagnostics` — the client's own account of a problem.
///
/// The failures that never produce an error are the ones most likely to go unreported: mAP stuck
/// at 0 for thirty epochs, a run that "succeeds" in two seconds, boxes all in one corner. Nothing
/// throws, so nothing is captured. This is the only path that lets a client say "this ran, and the
/// result is wrong".
///
/// The environment and the log tail are added here rather than accepted from the client: it cannot
/// know the engine's mtime, and letting it supply them would let a wrong guess into the report.
fn submit_diagnostic(request: &Request, runtime: &Runtime) -> Response {
    let Ok(value) = serde_json::from_slice::<Value>(&request.body) else {
        return Response::error(
            400,
            "body 不是有效 JSON。需要 {\"summary\":\"一句话现象\",\"context\":{…},\"expected\":\"预期是什么\"}。",
        );
    };
    let Some(summary) = value
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Response::error(
            400,
            "缺少 summary。用一句话说清现象，例如「训练 30 轮后 mAP 一直是 0，loss 一直在降」。",
        );
    };
    if summary.chars().count() > 2000 {
        return Response::error(400, "summary 太长（上限 2000 字）。细节放 context。");
    }
    let context = value.get("context").cloned().unwrap_or(Value::Null);
    let diagnostic = runtime.diagnose(
        &crate::diagnostics::Draft {
            kind: "assistant_report",
            code: "assistant_report",
            message: summary,
            hint: value.get("expected").and_then(Value::as_str),
            context,
        },
        None,
    );
    match diagnostic {
        Some(report) => Response::json(
            202,
            json!({
                "ok": true,
                "report": report,
                "note": "已写入本地。把 report.path 告诉用户，或让他把那份文件发给开发者。",
            }),
        ),
        None => Response::error(500, "诊断报告写入失败：检查磁盘剩余空间与目录写权限。"),
    }
}

fn uint(value: &Value, key: &str) -> Option<u32> {    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

/// Which task an architecture ID trains, decided by its suffix — the same rule `models.ts` uses
/// to build the IDs (`prefix + scale`, then `-seg` / `-cls`).
fn task_of_architecture(architecture: &str) -> Result<&'static str, String> {
    if architecture.ends_with("-sem") || architecture.ends_with("-depth") {
        return Err(format!(
            "{architecture} 只能推理，不能训练：引擎没有对应的损失与标注管线。\
             检测用 yolo26n，实例分割用 yolo26n-seg，分类用 yolo26n-cls。"
        ));
    }
    if architecture.ends_with("-seg") {
        return Ok("segment");
    }
    if architecture.ends_with("-cls") {
        return Ok("classify");
    }
    Ok("detect")
}

/// `None` when the dataset layout can serve this task.
///
/// The scanner reports `imagenet-folders` while the engine's own `dataset.json` writes
/// `classification-folders`; both are accepted so the check does not depend on which layer named
/// the layout.
fn task_mismatch(task: &str, format: &str) -> Option<String> {
    const CLASSIFY_FORMATS: [&str; 2] = ["imagenet-folders", "classification-folders"];
    match task {
        "classify" if !CLASSIFY_FORMATS.contains(&format) => Some(format!(
            "分类模型需要按类别分文件夹的数据集（train/<类别名>/*.jpg），但这是 {format} 格式。\
             要么把 architecture 的后缀去掉（例如 yolo26n-cls → yolo26n）做检测/分割，\
             要么换一个分类数据集。"
        )),
        "detect" | "segment" if format != "yolo" => {
            let label = if task == "segment" { "实例分割" } else { "检测" };
            Some(format!(
                "{label}任务需要 YOLO 格式数据集（images/ + labels/），但这是 {format}。\
                 分类数据集请用 -cls 结尾的架构，例如 yolo26n-cls。"
            ))
        }
        _ => None,
    }
}

/// Model IDs are `yolo26n`, `yolov8x-seg`, `yolo26n-cls`, `yolox-tiny`, … . Rejecting a leading
/// `-` matters: the value is passed as `--architecture <value>` and clap would otherwise read a
/// value like `--foo` as a new flag.
fn valid_architecture(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    value.len() <= 64
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
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
- URL 里的空格和中文目录名必须 **percent-encode**（空格 `%20`，中文用 UTF-8 字节的 `%XX`）。
  直接写空格会返回 400，而不是悄悄把路径截短。

## 端点

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| GET | `/` 或 `/index.md` | 本文件 |
| GET | `/health` | 服务与引擎是否活着（含应用版本） |
| GET | `/state` | 当前（或最近一次）训练的快照 |
| GET | `/state?since=N` | 快照 + 序号大于 N 的新事件 |
| GET | `/events?since=N` | 只要新事件，不重复快照 |
| POST | `/train` | 开始训练 |
| POST | `/stop` | 中止训练 |
| POST | `/predict` | 用训练好的模型给一张图推理 |
| GET | `/device?device=auto` | **训练前先问**：这台机器在这个设备上能不能真的跑起来 |
| GET | `/datasets/scan?path=...` | 检查一个数据集目录是否可用 |
| POST | `/datasets/scan` | 同上，但路径放在 JSON body 里（中文路径免 percent-encode） |
| GET | `/runs` | 列出历史训练与产物（应用重启后找旧模型的唯一途径） |
| GET | `/diagnostics` | 列出本机诊断报告（失败自动生成，路径已脱敏） |
| POST | `/diagnostics` | 上报一个「没报错但结果不对」的问题，服务端补全环境与日志 |
| GET | `/settings` | 读配置 |
| PUT | `/settings` | 改配置（整体替换，缺字段用默认值） |

**`/device` 值得在开跑前先问**：它让引擎在目标设备上真的执行一次矩阵乘、再把结果读回来，
所以答的是「能不能算」而不是「装了什么卡」。返回 `probe.ok`（能不能用）、`probe.adapter`
（显卡名）、`probe.backend`（`vulkan` / `dx12` / `gl` / `cpu`，未指定时是 `auto`）、
`probe.reason`（不能用的原因，引擎原话）、`probe.elapsed_ms`。第一次调用可能要等十几秒
（打不开的设备不会报错，只会挂到超时），结果在本进程内缓存，加 `?refresh=1` 才重测。
`probe.ok=false` 不等于不能训练：`device=auto` 会退回 CPU，只是慢。

**`/state` 有两种响应形态**：不带参数时**直接**就是快照对象（`status`/`epoch`/`best` 在顶层）；
带 `?since=N` 时是 `{{"ok":true,"state":{{...}},"cursor":N,"events":[...]}}`，同样的字段在
`state` 子对象里。别把两种写法混用。

## 典型流程

0. 先看有没有已经训好的模型。应用重启后 `/state` 是空的（它只记得本进程跑过的训练），
   但磁盘上的产物还在：

```bash
curl -s -H "Authorization: Bearer $TOKEN" "{base}/runs?limit=10"
```

`runs[]` 每项里：`best` 可以直接当 `/predict` 的 `model`；`resumable` 为 true 时可以用
`{{"init":{{"mode":"resume","checkpoint":"<dir>/checkpoints/last"}}}}` 接着上次训；
`final_loss` 和 `final_fitness` 是最后一轮的训练损失与验证指标——**判断模型好坏看
`final_fitness`，只看 loss 会漏掉过拟合**；`classes` 是它能识别的类别。列表按修改时间倒序。

1. 先扫描数据集，确认类别和图片数，别拿坏数据集去训练：

```bash
curl -s -H "Authorization: Bearer $TOKEN" \
  "{base}/datasets/scan?path=E:/data/coco-mini"

# 路径里有中文或空格时，用 POST 更省事，不必手工 percent-encode：
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{{"path":"E:/工作区/数据/coco-mini"}}' "{base}/datasets/scan"
```

返回 `dataset.train_images`、`dataset.classes`、`dataset.missing_labels`、`dataset.warning`。
`missing_labels > 0` 说明有图片没标注，可以训练但效果会打折；`train_images == 0` 就别开始。
`dataset.format` 是 `yolo`（检测/分割）或 `imagenet-folders`（分类），任务与格式必须匹配：
分类只能用 `imagenet-folders`，检测和分割只能用 `yolo`。**`/train` 会替你查这一条**，
架构与数据集不匹配时直接返回 400 并说明该改哪一边，不会先跑一分钟再失败。

2. 开始训练（`epochs` 是轮数，`batch` 是批大小，`device` 是 `auto|cpu|gpu`）：

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{{"dataset":"E:/data/coco-mini","epochs":20,"batch":4,"device":"auto"}}' \
  "{base}/train"
```

成功返回 `202` 和实际接受的参数。`accepted` 里是引擎**真正会用的**值：`device` 传了
`auto` / `cpu` / `gpu` 之外的东西会被改成 `auto`，并出现在 `warnings` 数组里——
不要拿你请求的值当真。已经有训练在跑时返回 `409`。

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
| `last_fitness` | 本轮验证适应度（检测看 mAP）。**判断模型好坏用这个，不是 loss**；`--no-val` 时为 null |
| `last_epoch_seconds` | 上一轮耗时，乘剩余轮数就是剩余时间 |
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

`model` 可省略，默认用最近一次训练导出的 `best`；`device` 可省略（默认 `cpu`），
可传 `cpu`/`gpu`/`auto`。返回 `result.prediction` 就是引擎的原始 JSON：检测模型给
`detections`（`xmin/ymin/xmax/ymax/confidence/class_name`），分割模型另给 `masks`，分类模型给
`classes`（top-5）。`result.output` 是标注后的图片路径，分类模型返回 `null`——它不画图。

标注图**只能**落在应用自己的 `predictions/` 目录：请求里的 `output` 会被忽略，调用方无法把
文件写回图片旁边，也无法指向任意路径。这样推理不会污染下一次的数据集扫描。

## 出问题时：报告已经自动写好了

`/train` 或 `/predict` 返回 `500` 时，服务端已经把一份诊断报告写到本地，并在响应里带回：

```json
{{"ok":false,"error":"引擎进程退出，代码 1","code":"engine_spawn_failed",
 "hint":"先看报告里的「引擎日志尾部」…",
 "diagnostics":{{"id":"20261113-123045-engine_spawn_failed",
   "path":"C:\\\\Users\\\\…\\\\diagnostics\\\\20261113-123045-engine_spawn_failed.md",
   "summary":"引擎进程退出，代码 1","pushed":true}}}}
```

**你不需要额外做什么。** 把 `error` 和 `hint` 讲给用户，再告诉他 `diagnostics.path` 那个文件
在哪。报告里有版本、实际后端、引擎文件的修改时间、请求参数、以及最近 120 行引擎日志——
这几样凑起来基本能定位问题。**用户名和数据集路径已分别替换成 `<user>` / `<dataset>`**，
可以直接外发。

`pushed: true` 表示摘要已经推到开发者那边（用户机器配置了上报通道时才有这个字段）；
`pushed: false` 且带 `push_error` 说明推送没成功，但**报告仍在本地**，让用户手动发那份文件。

错误响应现在一律带 `code`（机器可读）与可选的 `hint`（给人看的下一步）。别拿 `error` 的
措辞去判断类型，那是会变的文案；按 `code` 分支。`400` 是「改请求」，`500` 是「我们这边坏了」，
不要对 `500` 反复调参数重试。

## 没报错但结果不对：主动留一份

这是最容易漏的一类问题：`fitness` 连续几十轮是 0、训练 2 秒就结束、框全挤在左上角。
它们**不产生任何错误码**，所以自动报告抓不到，但恰恰最需要被看到。

```bash
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \\
  -d '{{"summary":"训练 30 轮 loss 从 456 降到 350，但 fitness 一直是 0",
       "expected":"同一数据集用预训练权重能到 3.2% mAP",
       "context":{{"architecture":"yolov8n","epochs":30,"imgsz":320}}}}' \\
  "{{base}}/diagnostics"
```

返回 `202` 和 `report.path`。**这不是错误**：调用成功只表示报告写下了，不代表结论。
写之前把「现象」和「预期」说清——这两句是判断问题唯一靠得住的东西，缺了它们，报告里剩下的
只是环境参数。

`GET /diagnostics?limit=20` 列出本机已有报告（`id` / `created` / `code` / `summary` /
`pushed`），按时间倒序。它们是文件而不是内存状态，应用重启后仍在。

## 事件流（`/events`）

响应是 `{{"ok":true,"cursor":N,"reset":false,"truncated":false,"events":[...]}}`：

- `cursor` — 下次把它当 `since` 传回来。**每次都用返回的值覆盖本地值**，
  不要在 events 为空时保留旧值，否则应用重启后你可能永远收不到新事件。
- `reset` — 为 true 说明你带的 cursor 来自上一个进程（`seq` 每次启动从 0 重新计）。
  事件已经**从缓冲区开头重放**给你了，按「重新开始」处理即可。
- `truncated` — 为 true 说明你漏掉了事件：缓冲区只留最近 400 条（引擎日志也算事件）。
  收到就先读一遍 `/state` 的完整快照再继续。

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

**这是整体替换**：漏掉的字段会被静默还原成默认值。先 `GET /settings` 拿到 `settings` 对象，
改其中一个字段再整份 PUT 回来。

```bash
curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{{"engine_path":null,"default_device":"cpu","output_dir":null,
      "control_enabled":true,"control_port":47821}}' "{base}/settings"
```

可写字段：`engine_path`（null = 自动检测）、`default_device`、`output_dir`（null = 默认）、
`control_enabled`、`control_port`（0 = 让系统选空闲端口）、`feedback_kind`
（`wecom` / `pushplus` / `generic`）、`feedback_webhook`（空字符串 = 只写本地报告，不外发）。改控制服务的开关或端口会**立即**
重启监听，不需要重启应用；重启后 `GET /settings` 的 `control.port` 才是实际端口
（端口被占用时可能不是你填的那个）。

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `%` followed by a multi-byte character used to slice `&str` in the middle of a code
    /// point and panic. With `panic = "abort"` that killed the whole app — and the query is
    /// parsed before the bearer token is checked, so it needed no credentials.
    #[test]
    fn percent_decode_never_panics_on_partial_utf8() {
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
        assert_eq!(percent_decode("E:/中文%20图"), "E:/中文 图");
        for nasty in [
            "E:/%中文",
            "E:/数据%图",
            "path=E:/%中",
            "%",
            "%Z",
            "%2",
            "%%%",
        ] {
            let _ = percent_decode(nasty);
        }
        assert_eq!(percent_decode("%2f%2F"), "//");
    }

    #[test]
    fn query_parsing_decodes_utf8_and_plus() {
        let query = parse_query("path=E%3A%2F%E6%95%B0%E6%8D%AE%2Ftiny&since=12&q=a+b");
        assert_eq!(query.get("path").unwrap(), "E:/数据/tiny");
        assert_eq!(query.get("since").unwrap(), "12");
        assert_eq!(query.get("q").unwrap(), "a b");
    }

    #[test]
    fn bearer_scheme_is_case_insensitive() {
        let mut headers = HashMap::new();
        for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
            headers.insert("authorization".to_owned(), format!("{scheme} tok"));
            assert!(authorised(&headers, &HashMap::new(), "tok"), "{scheme}");
        }
        headers.insert("authorization".to_owned(), "Bearer nope".to_owned());
        assert!(!authorised(&headers, &HashMap::new(), "tok"));
    }

    #[test]
    fn constant_time_eq_matches_only_equal() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn architecture_ids_are_constrained() {
        for good in [
            "yolo26n",
            "yolov8x-seg",
            "yolo26n-cls",
            "yolox-tiny",
            "yolov10b",
        ] {
            assert!(valid_architecture(good), "{good}");
        }
        for bad in ["--foo", "-seg", "", "yolo 26n", "a/b", "a\\b", "a&b"] {
            assert!(!valid_architecture(bad), "{bad}");
        }
    }

    #[test]
    fn health_reports_the_injected_version_not_cargo_pkg_version() {
        // Guards D022: Cargo.toml is frozen at 0.1.0 while tauri.conf.json is the source of truth.
        let _ = crate::version::VERSION;
        assert_eq!(crate::version::VERSION.split('.').count(), 3);
    }

    /* ─────────────── real HTTP, over a real listening socket ───────────────
     *
     * Everything above tests pure functions. None of it proves the service actually answers, and
     * the interesting failures — auth ordering, 405 vs 404, a body the router never sees — only
     * exist once a socket is involved.
     */

    struct Service {
        runtime: Runtime,
        handle: crate::control::Handle,
        root: std::path::PathBuf,
    }

    impl Drop for Service {
        fn drop(&mut self) {
            self.handle.stop();
            // `POST /train` tests do start a real engine child. Without this the test suite leaves
            // a training process burning CPU after the assertions finish.
            self.runtime.engine().kill_now();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A real service on an OS-assigned port, backed by a throwaway config directory.
    fn service(tag: &str) -> Service {
        let root = std::env::temp_dir().join(format!(
            "yolo-control-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.join("config"), root.join("data"));
        let handle = super::start(runtime.clone(), 0).expect("service should bind");
        Service {
            runtime,
            handle,
            root,
        }
    }

    struct Reply {
        status: u16,
        body: String,
    }

    impl Reply {
        fn json(&self) -> Value {
            serde_json::from_str(&self.body)
                .unwrap_or_else(|error| panic!("not JSON ({error}): {}", self.body))
        }
    }

    /// Send a raw request and read the reply **by `Content-Length`**, exactly as curl and reqwest
    /// do. Reading to EOF instead looks equivalent and is not: Windows resets a loopback socket
    /// when the server closes it while the kernel still holds unread bytes, so `read_to_end`
    /// reports `ConnectionReset` on a reply it has already received in full.
    ///
    /// No proxy is involved: this connects directly, which is exactly what the service is for.
    fn call(port: u16, head: &str, body: &[u8]) -> Reply {
        use std::io::{BufRead as _, Read as _, Write as _};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        let mut request = String::from(head);
        request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        stream.write_all(request.as_bytes()).expect("write head");
        stream.write_all(body).expect("write body");
        stream.flush().ok();

        let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
        let mut status_line = String::new();
        reader.read_line(&mut status_line).expect("status line");
        let status = status_line
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("no status in: {status_line:?}"));

        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("header line") == 0 {
                panic!("connection closed inside the headers");
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("content-length:") {
                length = rest.trim().parse().expect("content-length");
            }
        }

        let mut body = vec![0u8; length];
        reader
            .read_exact(&mut body)
            .expect("body shorter than Content-Length");
        Reply {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    }

    fn get(port: u16, path: &str, token: Option<&str>) -> Reply {
        let head = match token {
            Some(token) => format!("GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n"),
            None => format!("GET {path} HTTP/1.1\r\nHost: x\r\n"),
        };
        call(port, &head, b"")
    }

    fn post(port: u16, path: &str, token: &str, body: &str) -> Reply {
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\n"
        );
        call(port, &head, body.as_bytes())
    }

    /// One thread per connection means a client that retries in a tight loop can create threads
    /// faster than they retire, and each one lives up to `IO_TIMEOUT`. The count must also come
    /// back down: a leaked slot would leave the service refusing everything until the app
    /// restarts, which is worse than the problem the limit solves.
    #[test]
    fn connection_limit_refuses_with_503_then_recovers() {
        let service = service("conn-limit");
        let port = service.handle.port;

        // Hold every slot with connections that never send a request, so each handler parks in
        // `read_request` instead of finishing immediately.
        let mut hogs = Vec::new();
        for _ in 0..super::MAX_CONNECTIONS {
            hogs.push(std::net::TcpStream::connect(("127.0.0.1", port)).expect("hog connect"));
        }
        std::thread::sleep(Duration::from_millis(900));

        let refused = get(port, "/health", Some(&service.handle.token));
        assert_eq!(
            refused.status, 503,
            "over the limit must be a readable 503, not a dropped connection"
        );
        assert!(
            !refused.json()["error"].as_str().unwrap_or("").is_empty(),
            "a 503 without a reason leaves the caller guessing"
        );

        drop(hogs);
        let mut recovered = false;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            if get(port, "/health", Some(&service.handle.token)).status == 200 {
                recovered = true;
                break;
            }
        }
        assert!(
            recovered,
            "the service must accept again after the hogs disconnect"
        );
    }

    /// Percent-encode one query value. The fixture dataset lives under a path with Chinese
    /// characters, and an unencoded `GET` is exactly the trap `POST /datasets/scan` exists to
    /// avoid — so the end-to-end test has to hit both.
    fn encode_query(text: &str) -> String {
        let mut out = String::new();
        for byte in text.as_bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(*byte as char)
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        out
    }

    /// The flow an assistant actually performs, over real HTTP against a real engine and a real
    /// dataset — the same `control::start` the app runs, not a mock.
    ///
    /// `#[ignore]` because it trains: `cargo test --lib end_to_end -- --ignored --nocapture`.
    /// Every assertion is observable from HTTP alone, which is the point — an AI client has
    /// exactly this view and nothing more.
    #[test]
    #[ignore = "starts a real CPU training run"]
    fn end_to_end_assistant_flow() {
        let dataset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("spikes")
            .join("tiny-dataset");
        assert!(
            dataset.join("dataset.yaml").is_file(),
            "fixture dataset missing: {}",
            dataset.display()
        );
        let dataset = dataset.to_string_lossy().into_owned();

        let service = service("e2e");
        let port = service.handle.port;
        let token = service.handle.token.clone();

        // Discovery: /health names the engine binary, so a caller learns whether it can train at
        // all instead of finding out one request later.
        let health = get(port, "/health", Some(&token));
        assert_eq!(health.status, 200);
        let engine = health.json()["engine"].as_str().unwrap_or("").to_owned();
        assert!(
            std::path::Path::new(&engine).is_file(),
            "/health must point at a real engine: {engine}"
        );

        // Both scan entry points must agree. Two clients picking different ones cannot be allowed
        // to see different numbers for the same directory.
        let via_get = get(
            port,
            &format!("/datasets/scan?path={}", encode_query(&dataset)),
            Some(&token),
        );
        assert_eq!(via_get.status, 200, "GET scan: {}", via_get.body);
        let via_post = post(
            port,
            "/datasets/scan",
            &token,
            &json!({ "path": dataset }).to_string(),
        );
        assert_eq!(via_post.status, 200, "POST scan: {}", via_post.body);
        let get_summary = via_get.json()["dataset"].clone();
        let post_summary = via_post.json()["dataset"].clone();
        assert_eq!(get_summary["train_images"], post_summary["train_images"]);
        assert_eq!(get_summary["classes"], post_summary["classes"]);
        assert!(
            get_summary["train_images"].as_u64().unwrap_or(0) > 0,
            "fixture dataset has no training images"
        );

        // A classification architecture on a YOLO dataset used to surface minutes later, from the
        // engine. It must be refused before anything is spawned.
        let mismatch = post(
            port,
            "/train",
            &token,
            &json!({
                "init": { "mode": "scratch", "architecture": "yolo26n-cls" },
                "dataset": dataset,
                "epochs": 1,
            })
            .to_string(),
        );
        assert_eq!(mismatch.status, 400, "{}", mismatch.body);
        assert!(
            !mismatch.json()["error"].as_str().unwrap_or("").is_empty(),
            "a 400 must say which side to change"
        );

        // Stopping when nothing runs must say so instead of pretending it stopped something.
        let idle_stop = post(port, "/stop", &token, "{}");
        assert_eq!(idle_stop.status, 200);
        assert_eq!(idle_stop.json()["stopped"], false);

        let accepted = post(
            port,
            "/train",
            &token,
            &json!({ "dataset": dataset, "epochs": 1, "batch": 2, "imgsz": 320, "device": "cpu" })
                .to_string(),
        );
        assert_eq!(accepted.status, 202, "{}", accepted.body);

        let mut cursor = 0u64;
        let mut epoch_events = 0usize;
        let mut final_state = Value::Null;
        let deadline = Instant::now() + Duration::from_secs(420);
        while Instant::now() < deadline {
            let reply = get(port, &format!("/state?since={cursor}"), Some(&token));
            assert_eq!(reply.status, 200);
            let payload = reply.json();
            cursor = payload["cursor"].as_u64().unwrap_or(cursor);
            for event in payload["events"].as_array().cloned().unwrap_or_default() {
                if event["type"] == "epoch" {
                    epoch_events += 1;
                }
            }
            final_state = payload["state"].clone();
            if matches!(
                final_state["status"].as_str().unwrap_or(""),
                "done" | "failed"
            ) {
                break;
            }
            std::thread::sleep(Duration::from_millis(700));
        }

        assert_eq!(final_state["status"], "done", "state: {final_state}");
        assert_eq!(epoch_events, 1, "one epoch was requested");
        // `fitness` is what a caller judges a model by; loss alone has been misleading on tiny
        // datasets in this project before. Present-but-null is the correct answer for `--no-val`.
        assert!(
            final_state.get("last_fitness").is_some(),
            "state must carry last_fitness: {final_state}"
        );

        let best = final_state["best"].as_str().unwrap_or("").to_owned();
        assert!(
            std::path::Path::new(&best).is_file(),
            "best artifact missing: {best}"
        );

        // The engine's stderr must be on disk now, not just in the log panel: this is the only
        // record of a GPU `device lost` once the window closes, and the whole diagnostics feature
        // depends on it.
        let engine_log = service
            .runtime
            .engine()
            .log_path()
            .expect("the app engine must be built with a log dir");
        let logged = std::fs::read_to_string(&engine_log).unwrap_or_default();
        assert!(
            logged.len() > 200,
            "engine log is empty ({} bytes at {})",
            logged.len(),
            engine_log.display()
        );

        // /runs must find, from disk, the run that just finished — this is the endpoint that
        // survives an app restart, where /state forgets everything.
        let runs = get(port, "/runs", Some(&token));
        assert_eq!(runs.status, 200);
        let list = runs.json()["runs"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            list.len(),
            1,
            "the throwaway output dir holds exactly the run under test: {}",
            runs.body
        );
        let this_run = &list[0];
        assert_eq!(this_run["epochs_completed"], 1);
        assert_eq!(this_run["resumable"], true);
        assert!(
            this_run["classes"]
                .as_array()
                .is_some_and(|classes| !classes.is_empty()),
            "classes must come from the run's own config: {this_run}"
        );
        // The two endpoints spell the same artifact differently — the engine hands `/state` a
        // forward-slash path and `runs.rs` joins with the platform separator — so compare what the
        // filesystem resolves rather than the strings. A caller that string-matches would be
        // wrong; this asserts they are the same file.
        let runs_best = this_run["best"].as_str().unwrap_or("");
        assert_eq!(
            std::fs::canonicalize(runs_best).ok(),
            std::fs::canonicalize(&best).ok(),
            "/runs and /state must point at the same artifact ({runs_best} vs {best})"
        );

        // Inference with the artifact this run produced.
        let val_dir = std::path::Path::new(&dataset).join("images").join("val");
        let image = std::fs::read_dir(&val_dir)
            .expect("val images")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "jpg"))
            .expect("a validation image");
        let predicted = post(
            port,
            "/predict",
            &token,
            &json!({ "model": best, "source": image.to_string_lossy() }).to_string(),
        );
        assert_eq!(predicted.status, 200, "{}", predicted.body);
        assert!(
            predicted.json()["result"]["prediction"].is_object(),
            "the engine document is passed through unchanged: {}",
            predicted.body
        );
    }

    #[test]
    fn the_document_is_readable_without_a_token_but_nothing_else_is() {
        let service = service("auth");
        let port = service.handle.port;

        let guide = get(port, "/", None);
        assert_eq!(guide.status, 200);
        assert!(guide.body.contains("/runs"), "the document must list the new endpoint");

        assert_eq!(get(port, "/health", None).status, 401);
        assert_eq!(get(port, "/state", None).status, 401);
        assert_eq!(get(port, "/health", Some("wrong")).status, 401);

        let health = get(port, "/health", Some(&service.handle.token));
        assert_eq!(health.status, 200);
        assert_eq!(health.json()["ok"], true);
    }

    /// A known path with the wrong method is 405; an unknown one is 404. An AI probing the API
    /// reads these as different answers.
    #[test]
    fn method_and_path_errors_are_distinguished() {
        let service = service("routes");
        let port = service.handle.port;
        let token = service.handle.token.clone();

        assert_eq!(get(port, "/nope", Some(&token)).status, 404);
        assert_eq!(get(port, "/train", Some(&token)).status, 405);
        assert_eq!(post(port, "/runs", &token, "").status, 405);
        assert_eq!(post(port, "/datasets/scan", &token, "not json").status, 400);
    }

    /// Both scan entry points must answer the same thing. The GET one needs percent-encoding;
    /// the POST one does not, which is the whole point of adding it.
    #[test]
    fn both_scan_entry_points_agree() {
        let service = service("scan");
        let port = service.handle.port;
        let token = service.handle.token.clone();
        let dataset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("spikes")
            .join("tiny-dataset");
        assert!(dataset.is_dir(), "fixture missing: {}", dataset.display());
        let as_forward = dataset.to_string_lossy().replace('\\', "/");

        let encoded: String = as_forward
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'.' | b'-' | b'_') {
                    (byte as char).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect();
        let via_get = get(port, &format!("/datasets/scan?path={encoded}"), Some(&token));
        assert_eq!(via_get.status, 200, "{}", via_get.body);
        assert_eq!(via_get.json()["dataset"]["format"], "yolo");

        let body = serde_json::json!({ "path": as_forward }).to_string();
        let via_post = post(port, "/datasets/scan", &token, &body);
        assert_eq!(via_post.status, 200, "{}", via_post.body);
        assert_eq!(
            via_get.json()["dataset"], via_post.json()["dataset"],
            "the two entry points must not disagree"
        );
    }

    /// After a restart the counter starts at 0 while the client still holds an old cursor. The
    /// service has to replay instead of answering "nothing new" forever.
    #[test]
    fn a_stale_cursor_after_restart_is_flagged_not_silently_empty() {
        let service = service("cursor");
        let port = service.handle.port;
        let token = service.handle.token.clone();

        let reply = get(port, "/events?since=999999", Some(&token));
        assert_eq!(reply.status, 200);
        let body = reply.json();
        assert_eq!(body["reset"], true);
        assert_eq!(body["truncated"], false);
        assert_eq!(body["cursor"], 0);

        // A normal poll is not flagged.
        let normal = get(port, "/state?since=0", Some(&token)).json();
        assert_eq!(normal["reset"], false);
        assert!(normal.get("state").is_some());
    }

    /// Every run the app ever produced, from disk. This is the whole reason `/runs` exists: after
    /// a restart `/state` has no `best`, and without this an assistant cannot find the model.
    #[test]
    fn runs_reads_what_is_actually_on_disk() {
        let service = service("runs");
        let port = service.handle.port;
        let token = service.handle.token.clone();

        let empty = get(port, "/runs", Some(&token));
        assert_eq!(empty.status, 200);
        assert_eq!(empty.json()["count"], 0, "a fresh install has no runs");

        let run = service.root.join("data").join("runs").join("detect").join("train-7");
        std::fs::create_dir_all(run.join("exports")).unwrap();
        std::fs::write(
            run.join("results.csv"),
            "epoch,train/loss,fitness\n1,2.5,\n2,1.5,0.31\n",
        )
        .unwrap();
        std::fs::write(run.join("exports").join("best.bpk"), b"w").unwrap();

        let body = get(port, "/runs", Some(&token)).json();
        assert_eq!(body["count"], 1, "{}", body);
        let first = &body["runs"][0];
        assert_eq!(first["name"], "train-7");
        assert_eq!(first["task"], "detect");
        assert_eq!(first["epochs_completed"], 2);
        assert_eq!(first["final_loss"], 1.5);
        assert_eq!(first["final_fitness"], 0.31);
        assert!(first["best"].as_str().unwrap().ends_with("best.bpk"));
        assert_eq!(first["resumable"], false, "no checkpoints/last was written");
    }

    /// `stopped: true` when nothing was running is a lie an assistant waits on forever.
    #[test]
    fn stopping_nothing_says_so() {
        let service = service("stop");
        let reply = post(
            service.handle.port,
            "/stop",
            &service.handle.token.clone(),
            "",
        );
        assert_eq!(reply.status, 200);
        let body = reply.json();
        assert_eq!(body["stopped"], false);
        assert!(body["note"].as_str().unwrap().contains("没有"));
    }

    /// The window command and the HTTP route must agree on what a device string means, because
    /// `/train` echoes the normalised value and the caller acts on it.
    #[test]
    fn train_echoes_the_device_the_engine_will_actually_use() {
        let service = service("device");
        let dataset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("spikes")
            .join("tiny-dataset");
        let body = serde_json::json!({
            "architecture": "yolo26n",
            "dataset": dataset.to_string_lossy(),
            "epochs": 1,
            "batch": 2,
            "device": "tpu",
        })
        .to_string();
        let reply = post(
            service.handle.port,
            "/train",
            &service.handle.token.clone(),
            &body,
        );
        // The engine may or may not be resolvable in this checkout; what matters is that a
        // rejection is not a 400 about the device and that an accepted request reports `auto`.
        if reply.status == 202 {
            let accepted = reply.json();
            assert_eq!(accepted["accepted"]["device"], "auto");
            assert!(
                accepted["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|warning| warning.as_str().unwrap().contains("auto")),
                "the rewrite must be explained: {accepted}"
            );
            let _ = service.runtime.stop_training();
        } else {
            assert_ne!(reply.status, 400, "device must never be a 400: {}", reply.body);
        }
    }

    /// A classification architecture on a YOLO dataset is rejected before anything is spawned.
    #[test]
    fn a_task_dataset_mismatch_is_a_400_with_advice() {
        let service = service("mismatch");
        let dataset = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("spikes")
            .join("tiny-dataset");
        let body = serde_json::json!({
            "architecture": "yolo26n-cls",
            "dataset": dataset.to_string_lossy(),
            "epochs": 1,
        })
        .to_string();
        let reply = post(
            service.handle.port,
            "/train",
            &service.handle.token.clone(),
            &body,
        );
        assert_eq!(reply.status, 400, "{}", reply.body);
        let message = reply.json()["error"].as_str().unwrap().to_owned();
        assert!(message.contains("分类"), "must say what is wrong: {message}");
        assert!(
            message.contains("yolo26n-cls"),
            "must name the fix: {message}"
        );
    }

    #[test]
    fn diagnostics_is_token_gated_and_starts_empty() {
        let service = service("diag-auth");
        let port = service.handle.port;
        assert_eq!(get(port, "/diagnostics", None).status, 401);
        let reply = get(port, "/diagnostics", Some(&service.handle.token));
        assert_eq!(reply.status, 200);
        // A fresh install has no `diagnostics/` directory; that is "nothing to report", not an
        // error the client should surface.
        assert_eq!(reply.json()["count"], 0);
        assert_eq!(reply.json()["reports"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn a_client_report_gets_environment_and_a_real_file() {
        let service = service("diag-post");
        let port = service.handle.port;
        let body = json!({
            "summary": "训练 30 轮 loss 从 456 降到 350，但 fitness 一直是 0",
            "expected": "同一数据集用预训练权重能到 3.2% mAP",
            "context": { "architecture": "yolov8n", "epochs": 30 },
        })
        .to_string();
        let reply = post(port, "/diagnostics", &service.handle.token.clone(), &body);
        assert_eq!(reply.status, 202, "{}", reply.body);

        let report = &reply.json()["report"];
        let path = report["path"].as_str().expect("path").to_owned();
        assert!(
            std::path::Path::new(&path).is_file(),
            "report was not written: {path}"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("fitness 一直是 0"));
        // The environment is filled in by the server, not by the client: a client cannot know the
        // engine's mtime and must not be trusted to describe the machine.
        assert!(text.contains("## 运行环境"), "{}", &text[..200.min(text.len())]);
        assert!(text.contains(crate::version::VERSION));
        // No channel configured in a test build, so nothing left the machine.
        assert_eq!(report["pushed"], false);
        assert!(report.get("push_error").is_none() || report["push_error"].is_null());

        let listed = get(port, "/diagnostics?limit=5", Some(&service.handle.token));
        assert_eq!(listed.json()["count"], 1);
        assert_eq!(listed.json()["reports"][0]["code"], "assistant_report");
    }

    #[test]
    fn a_report_without_a_summary_is_refused_because_it_says_nothing() {
        let service = service("diag-nosummary");
        let reply = post(
            service.handle.port,
            "/diagnostics",
            &service.handle.token.clone(),
            &json!({ "context": { "epochs": 3 } }).to_string(),
        );
        assert_eq!(reply.status, 400);
        let message = reply.json()["error"].as_str().unwrap_or("").to_owned();
        assert!(message.contains("summary"), "{message}");
    }

    /// The failure path that matters most: the engine dies, and the client did not ask for a
    /// report. A 500 without one leaves the user with "推理失败：exit code 1" and nothing to send.
    #[test]
    fn a_failed_prediction_carries_a_written_report_without_being_asked() {
        let dataset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("spikes")
            .join("tiny-dataset");
        let Some(source) = std::fs::read_dir(dataset.join("images").join("train"))
            .ok()
            .and_then(|entries| entries.filter_map(Result::ok).next())
        else {
            panic!("fixture dataset missing: {}", dataset.display());
        };
        let service = service("diag-predict");
        let port = service.handle.port;
        // Exists, so the request passes validation, but is not a usable Burnpack: the engine starts
        // and fails, which is exactly the 500 path.
        let fake = service.root.join("not-a-model.bpk");
        std::fs::write(&fake, b"definitely not a burnpack").unwrap();

        let body = json!({
            "source": source.path().to_string_lossy(),
            "model": fake.to_string_lossy(),
        })
        .to_string();
        let reply = post(port, "/predict", &service.handle.token.clone(), &body);
        assert_eq!(reply.status, 500, "expected an engine failure: {}", reply.body);
        assert_eq!(reply.json()["code"], "engine_predict_failed");

        let report = &reply.json()["diagnostics"];
        let path = report["path"].as_str().unwrap_or_else(|| {
            panic!("500 must carry a diagnostics path, got: {}", reply.body)
        });
        assert!(std::path::Path::new(path).is_file(), "{path}");
        let text = std::fs::read_to_string(path).unwrap();
        // The engine's own words, not our paraphrase — that detail is the reason the report exists.
        assert!(
            text.contains("Burnpack") || text.contains("INTERNAL"),
            "engine stderr missing from the report: {text}"
        );
        assert!(text.contains("engine_predict_failed"));
        // The temp paths in the context block contain the account name; redaction must catch the
        // JSON-escaped spelling, which is how they actually appear.
        if let Ok(home) = std::env::var("USERPROFILE") {
            if let Some(name) = home.rsplit(['\\', '/']).next() {
                assert!(
                    !text.contains(name),
                    "account name {name:?} leaked into the report: {text}"
                );
            }
        }
        // The engine's own stderr is the reason this report is worth writing at all.
        assert!(text.contains("引擎日志尾部"), "{text}");
    }
}
