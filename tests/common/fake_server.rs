//! A real HTTP server on a local port that behaves like the API's device routes: it pairs
//! devices and checks request signatures in the order the spec lists them.
//!
//! It deliberately does not use the crate's own `auth` module. It builds the signed text
//! by hand from the spec, so a mistake in the client's code is not repeated here.
// Each test binary that includes this file uses a part of it.
#![allow(dead_code)]
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{Signature, VerifyingKey};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

pub const WINDOW_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Device {
    key: VerifyingKey,
    pub active: bool,
    pub revoked_reason: Option<String>,
    installation_id: String,
}

#[derive(Default)]
pub struct State {
    pub devices: HashMap<String, Device>,
    /// (device, nonce): what the nonce check remembers.
    nonces: Vec<(String, String)>,
    /// How far this server's clock is from the real one.
    pub clock_offset_ms: i64,
    pub seen: Vec<Seen>,
    /// Where `/redirect` sends the caller.
    pub redirect_to: String,
    pairing: Option<(String, [u8; 32], bool, String)>,
    next_device: u32,
    /// The server's own address, for the links it hands out.
    base: String,
    /// How the next pairing goes. By default it waits for `approve()`.
    pub script: Script,
    polls: usize,
    /// The reply to `GET /api/cli/v1/status`, apart from the device (that one is computed).
    pub organization_name: String,
    pub sources: Vec<serde_json::Value>,
    pub collection: Vec<serde_json::Value>,
    /// How `POST /api/ingest/v1/events` behaves, and what it was given.
    pub ingest: Ingest,
    pub otlp: Otlp,
    /// Every observation accepted, once per identity: what the API would have stored.
    pub stored: Vec<serde_json::Value>,
    pub stored_coverage: Vec<serde_json::Value>,
    /// One entry per batch that was understood: (observations, coverage, bytes on the wire).
    pub batches: Vec<(usize, usize, usize)>,
    identities: HashSet<(String, String, String)>,
    /// What the 9.4 `collection` is worked out from: for each source this device has sent.
    seen_at: HashMap<String, i64>,
    observed_until: HashMap<String, i64>,
    /// `(source, from, to, reason)` in the order reported, and the observed ranges after them.
    gaps: Vec<(String, i64, i64, String, usize)>,
    observed: Vec<(String, i64, i64, usize)>,
    reports: usize,
}

/// `POST /api/ingest/otlp/v1/logs`: what the relay sent, and how the route answers.
#[derive(Default)]
pub struct Otlp {
    /// Batch-level failures served first, one per request: (status, code).
    pub fail_next: VecDeque<(u16, String)>,
    /// Every payload that reached the route signed and well-formed: (device, JSON).
    pub received: Vec<(String, serde_json::Value)>,
    /// Batches counted in the current minute, per device: (start of the minute, batches).
    window: HashMap<String, (std::time::Instant, u32)>,
}

/// The route's own ceiling, apart from the `sync`'s.
const OTLP_PER_MINUTE: u32 = 60;

#[derive(Default)]
pub struct Ingest {
    /// Batch-level failures served first, one per request: (status, code, `Retry-After`).
    pub fail_next: VecDeque<(u16, String, Option<u64>)>,
    /// Per-item refusals by `externalEventId`.
    pub reject: HashMap<String, String>,
    /// An item from before this (ISO) is `BEFORE_COLLECTION_WINDOW`.
    pub window_start: Option<String>,
    /// Answer 200 with the wrong number of results.
    pub short_results: bool,
    /// Refuse bodies over this many bytes once decompressed (default 1 MiB).
    pub max_plain: Option<usize>,
}

/// What the person does in the browser, and when the server says so.
#[derive(Clone, Default)]
pub struct Script {
    /// Approve at this poll (1 = the first). `None`: wait for `approve()`.
    pub approve_at_poll: Option<usize>,
    /// Answer `slow_down` to this many polls first.
    pub slow_downs: usize,
    /// Answer `access_denied` / `expired_token` instead of approving.
    pub deny: bool,
    pub expire: bool,
    /// The browser refused the approval (the organization has authorized no source): the code
    /// stays valid and every poll says `authorization_pending`.
    pub refuse_approval: bool,
    /// The `interval` handed out (default 5).
    pub interval: Option<u64>,
    pub organization_id: Option<String>,
    pub organization_name: Option<String>,
    /// A status code to answer every poll with instead (a broken server).
    pub poll_status: Option<u16>,
    /// The verification link is on this origin instead of the server's own.
    pub link_origin: Option<String>,
    /// What `status` says about the device, instead of the real fingerprint and expiry.
    pub fingerprint: Option<String>,
    pub device_expires_at: Option<String>,
}

pub struct FakeServer {
    pub base: String,
    pub state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

type Reply = (u16, Vec<(String, String)>, Vec<u8>);

fn error(status: u16, code: &str) -> Reply {
    (
        status,
        vec![],
        format!(r#"{{"error":{{"code":"{code}","message":"text the client must not show"}}}}"#)
            .into_bytes(),
    )
}

fn json(status: u16, value: serde_json::Value) -> Reply {
    (status, vec![], value.to_string().into_bytes())
}

impl FakeServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            base: format!("http://127.0.0.1:{}", addr.port()),
            organization_name: "Acme Inc.".into(),
            ..State::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (state.clone(), stop.clone());
        thread::spawn(move || {
            for conn in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(conn) = conn {
                    let s = s.clone();
                    thread::spawn(move || serve(conn, &s));
                }
            }
        });
        Self {
            base: format!("http://127.0.0.1:{}", addr.port()),
            state,
            stop,
            addr,
        }
    }

    /// The person approved the pending pairing in their browser.
    pub fn approve(&self) -> String {
        let mut s = self.state.lock().unwrap();
        s.next_device += 1;
        let id = format!("dev-{}", s.next_device);
        approve_pending(&mut s, &id);
        id
    }

    /// Registers a device without a pairing, for tests that only need a known key.
    pub fn register(&self, id: &str, public_key: &str) {
        let bytes: [u8; 32] = B64.decode(public_key).unwrap().try_into().unwrap();
        self.state.lock().unwrap().devices.insert(
            id.into(),
            Device {
                key: VerifyingKey::from_bytes(&bytes).unwrap(),
                active: true,
                revoked_reason: None,
                installation_id: String::new(),
            },
        );
    }

    /// Changes how pairing goes and what `status` says.
    pub fn configure(&self, edit: impl FnOnce(&mut State)) {
        edit(&mut self.state.lock().unwrap());
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    pub fn device_active(&self, id: &str) -> bool {
        self.state.lock().unwrap().devices[id].active
    }

    pub fn revoked_reason(&self, id: &str) -> Option<String> {
        self.state.lock().unwrap().devices[id]
            .revoked_reason
            .clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr); // wake the accept loop
    }
}

fn serve(mut conn: TcpStream, state: &Mutex<State>) {
    let Some(req) = read_request(&mut conn) else {
        return;
    };
    let reply = state.lock().map(|mut s| route(&mut s, req)).unwrap();
    let (status, headers, body) = reply;
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !body.is_empty() {
        out.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = conn.write_all(out.as_bytes());
    let _ = conn.write_all(&body);
}

fn read_request(conn: &mut TcpStream) -> Option<Seen> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        let n = conn.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let want: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < want {
        let n = conn.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Seen {
        method,
        path,
        headers,
        body,
    })
}

fn route(s: &mut State, req: Seen) -> Reply {
    s.seen.push(req.clone());
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/api/cli/v1/device-codes") => start_pairing(s, &req.body),
        ("POST", "/api/cli/v1/tokens") => poll(s, &req.body),
        ("GET", "/api/cli/v1/status") => match authenticate(s, &req) {
            Ok(id) => status(s, &id),
            Err(reply) => reply,
        },
        ("POST", "/api/cli/v1/logout") => match authenticate(s, &req) {
            Ok(id) => {
                let d = s.devices.get_mut(&id).unwrap();
                (d.active, d.revoked_reason) = (false, Some("logout".into()));
                (204, vec![], vec![])
            }
            Err(reply) => reply,
        },
        ("POST", "/api/ingest/v1/events") => match authenticate(s, &req) {
            Ok(id) => ingest(s, &req, &id),
            Err(reply) => reply,
        },
        ("POST", "/api/ingest/otlp/v1/logs") => match authenticate(s, &req) {
            Ok(id) => otlp_logs(s, &req, &id),
            Err(reply) => reply,
        },
        ("GET", "/redirect") => (
            302,
            vec![("Location".into(), s.redirect_to.clone())],
            vec![],
        ),
        ("GET", "/limited") => {
            let mut r = error(429, "RATE_LIMITED");
            r.1.push(("Retry-After".into(), "60".into()));
            r
        }
        ("GET", "/broken") => (500, vec![], b"<html>oops</html>".to_vec()),
        ("GET", "/garbage") => (200, vec![], b"not json".to_vec()),
        ("GET", "/huge") => (200, vec![], vec![b'x'; 2 * 1024 * 1024]),
        _ => error(404, "NOT_FOUND"),
    }
}

fn start_pairing(s: &mut State, body: &[u8]) -> Reply {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return error(400, "VALIDATION_FAILED");
    };
    let Some(map) = v.as_object() else {
        return error(400, "VALIDATION_FAILED");
    };
    // strict: exactly these keys
    let mut keys: Vec<_> = map.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let key = map
        .get("publicKey")
        .and_then(|k| k.as_str())
        .and_then(|k| B64.decode(k).ok())
        .and_then(|k| <[u8; 32]>::try_from(k).ok());
    let (true, Some(key)) = (keys == ["cliVersion", "installationId", "publicKey"], key) else {
        return error(400, "VALIDATION_FAILED");
    };
    let installation_id = map["installationId"].as_str().unwrap_or("").to_owned();
    s.pairing = Some((
        "dc-abcdefghijklmnopqrstuvwxyz".into(),
        key,
        false,
        installation_id,
    ));
    s.polls = 0;
    let origin = s
        .script
        .link_origin
        .clone()
        .unwrap_or_else(|| s.base.clone());
    json(
        200,
        serde_json::json!({
            "deviceCode": "dc-abcdefghijklmnopqrstuvwxyz",
            "userCode": "BCDF-GHJK",
            "verificationUri": format!("{origin}/devices/pair"),
            "verificationUriComplete": format!("{origin}/devices/pair?code=BCDF-GHJK"),
            "expiresIn": 600,
            "interval": s.script.interval.unwrap_or(5)
        }),
    )
}

fn poll(s: &mut State, body: &[u8]) -> Reply {
    if let Some(status) = s.script.poll_status {
        return error(status, "UNAVAILABLE");
    }
    let code = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["deviceCode"].as_str().map(str::to_owned));
    let status = |st: &str| json(200, serde_json::json!({ "status": st }));
    let known = matches!((&s.pairing, &code), (Some((expected, ..)), Some(c)) if expected == c);
    if !known || s.script.expire {
        return status("expired_token");
    }
    s.polls += 1;
    if s.script.deny {
        return status("access_denied");
    }
    if s.script.refuse_approval {
        return status("authorization_pending");
    }
    let approved = s.pairing.as_ref().is_some_and(|p| p.2);
    if !approved && s.polls <= s.script.slow_downs {
        return status("slow_down");
    }
    if !approved && s.script.approve_at_poll.is_some_and(|n| s.polls >= n) {
        s.next_device += 1;
        let id = format!("dev-{}", s.next_device);
        approve_pending(s, &id);
    }
    if s.pairing.as_ref().is_some_and(|p| p.2) {
        let id = format!("dev-{}", s.next_device);
        json(
            200,
            serde_json::json!({
                "status": "approved", "deviceId": id,
                "expiresAt": "2027-02-08T00:00:00Z",
                "organizationId": s.script.organization_id.clone().unwrap_or("org_1".into()),
                "organizationName": s.script.organization_name.clone().unwrap_or(s.organization_name.clone())
            }),
        )
    } else {
        status("authorization_pending")
    }
}

/// The person approved: the device exists, and the one it replaces (same installation) is revoked.
fn approve_pending(s: &mut State, id: &str) {
    let (_, key, approved, installation_id) = s.pairing.as_mut().expect("no pairing started");
    *approved = true;
    let (key, installation_id) = (
        VerifyingKey::from_bytes(key).unwrap(),
        installation_id.clone(),
    );
    for d in s.devices.values_mut() {
        if d.active && d.installation_id == installation_id && !installation_id.is_empty() {
            (d.active, d.revoked_reason) = (false, Some("replaced".into()));
        }
    }
    s.devices.insert(
        id.to_owned(),
        Device {
            key,
            active: true,
            revoked_reason: None,
            installation_id,
        },
    );
}

/// `CliStatus` of the spec. The fingerprint is worked out here by hand, from the raw key.
fn status(s: &State, id: &str) -> Reply {
    let device = &s.devices[id];
    let hex: String = Sha256::digest(device.key.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    let fingerprint = format!(
        "{}-{}-{}-{}",
        &hex[0..4],
        &hex[4..8],
        &hex[8..12],
        &hex[12..16]
    );
    json(
        200,
        serde_json::json!({
            "organizationName": s.organization_name,
            "memberName": "Jane Doe",
            "device": {
                "label": "test-laptop",
                "keyFingerprint": s.script.fingerprint.clone().unwrap_or(fingerprint),
                "createdAt": "2026-11-12T10:00:00Z",
                "expiresAt": s.script.device_expires_at.clone().unwrap_or("2027-02-08T00:00:00Z".into())
            },
            "sources": s.sources,
            "collection": if s.collection.is_empty() { collection(s) } else { s.collection.clone() }
        }),
    )
}

/// The checks of the spec, in its order. Returns the device id or the reply to send.
fn authenticate(s: &mut State, req: &Seen) -> Result<String, Reply> {
    let header = |n: &str| req.headers.get(n).map(String::as_str);
    let unauth = || error(401, "UNAUTHENTICATED");
    // 1. the device exists and is active
    let (Some(id), Some(ts), Some(nonce), Some(sig)) = (
        header("x-sb-device"),
        header("x-sb-timestamp"),
        header("x-sb-nonce"),
        header("x-sb-signature"),
    ) else {
        return Err(unauth());
    };
    let Some(device) = s.devices.get(id).filter(|d| d.active) else {
        return Err(unauth());
    };
    // 2. the signature is valid for that device's key (no revocation if not)
    let mut hash = String::new();
    for b in Sha256::digest(&req.body) {
        hash.push_str(&format!("{b:02x}"));
    }
    let text = format!("sb-v1\n{}\n{}\n{ts}\n{nonce}\n{hash}", req.method, req.path);
    let valid = B64
        .decode(sig)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .is_some_and(|b| {
            device
                .key
                .verify_strict(text.as_bytes(), &Signature::from_bytes(&b))
                .is_ok()
        });
    if !valid {
        return Err(unauth());
    }
    // 3. the timestamp is within 5 minutes of the server's clock (no revocation)
    let server_now = now_ms() + s.clock_offset_ms;
    let in_window = ts
        .parse::<i64>()
        .is_ok_and(|t| (t - server_now).abs() <= WINDOW_MS);
    if !in_window {
        let mut r = error(401, "CLOCK_SKEW");
        r.1.push(("X-SB-Server-Time".into(), server_now.to_string()));
        return Err(r);
    }
    // 4. the nonce was not seen for this device: a repeat is refused
    let pair = (id.to_owned(), nonce.to_owned());
    if s.nonces.contains(&pair) {
        return Err(unauth());
    }
    s.nonces.push(pair);
    Ok(id.to_owned())
}

/// `POST /api/ingest/otlp/v1/logs`: its own ceiling of 60 batches a minute per device, JSON up to
/// 1 MiB, and the strict filter: a record with an attribute outside the declared twelve, or with a
/// key given twice, is refused whole.
fn otlp_logs(s: &mut State, req: &Seen, device: &str) -> Reply {
    const ALLOWED: [&str; 12] = [
        "event.name",
        "event.timestamp",
        "session.id",
        "request_id",
        "model",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_creation_tokens",
        "cache_creation_1h_tokens",
        "query_source",
        "organization.id",
    ];
    let now = std::time::Instant::now();
    let (start, count) = s.otlp.window.entry(device.to_owned()).or_insert((now, 0));
    if now.duration_since(*start).as_secs() >= 60 {
        (*start, *count) = (now, 0);
    }
    *count += 1;
    if *count > OTLP_PER_MINUTE {
        let wait = 60u64
            .saturating_sub(now.duration_since(*start).as_secs())
            .max(1);
        let mut reply = error(429, "RATE_LIMITED");
        reply.1.push(("Retry-After".into(), wait.to_string()));
        return reply;
    }
    if let Some((status, code)) = s.otlp.fail_next.pop_front() {
        return error(status, &code);
    }
    let header = |n: &str| req.headers.get(n).map(String::as_str);
    if header("content-type") != Some("application/json") || header("content-encoding").is_some() {
        return error(415, "UNSUPPORTED_MEDIA_TYPE");
    }
    if req.body.len() > 1024 * 1024 {
        return error(413, "PAYLOAD_TOO_LARGE");
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&req.body) else {
        return error(400, "VALIDATION_FAILED");
    };
    let mut refused = 0;
    for resource in v["resourceLogs"].as_array().into_iter().flatten() {
        for scope in resource["scopeLogs"].as_array().into_iter().flatten() {
            for record in scope["logRecords"].as_array().into_iter().flatten() {
                let keys: Vec<&str> = record["attributes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| a["key"].as_str().unwrap_or(""))
                    .collect();
                let outside = keys.iter().any(|k| !ALLOWED.contains(k));
                // A repeated key refuses the record, the same as one outside the twelve.
                let repeated = keys.iter().enumerate().any(|(i, k)| keys[..i].contains(k));
                refused += u64::from(outside || repeated);
            }
        }
    }
    s.otlp.received.push((device.to_owned(), v));
    if refused == 0 {
        json(200, serde_json::json!({}))
    } else {
        json(
            200,
            serde_json::json!({"partialSuccess": {"rejectedLogRecords": refused, "errorMessage": "x"}}),
        )
    }
}

/// `POST /api/ingest/v1/events` as the contract describes it: gzip, at most 1 MiB once
/// decompressed, strict shape, one result per observation in order.
fn ingest(s: &mut State, req: &Seen, device: &str) -> Reply {
    if let Some((status, code, retry)) = s.ingest.fail_next.pop_front() {
        let mut r = error(status, &code);
        if let Some(secs) = retry {
            r.1.push(("Retry-After".into(), secs.to_string()));
        }
        return r;
    }
    let header = |n: &str| req.headers.get(n).map(String::as_str);
    if header("content-type") != Some("application/json")
        || header("content-encoding") != Some("gzip")
    {
        return error(415, "UNSUPPORTED_MEDIA_TYPE");
    }
    let limit = s.ingest.max_plain.unwrap_or(1024 * 1024);
    let mut plain = Vec::new();
    let read = GzDecoder::new(&req.body[..])
        .take(limit as u64 + 1)
        .read_to_end(&mut plain);
    if read.is_err() {
        return error(400, "VALIDATION_FAILED");
    }
    // Over 1 MiB once decompressed, or more than 200 times what was sent (a bomb): the batch
    // is too big, and a smaller one gets through.
    if plain.len() > limit || plain.len() > req.body.len() * 200 {
        return error(413, "PAYLOAD_TOO_LARGE");
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&plain) else {
        return error(400, "VALIDATION_FAILED");
    };
    let Some(map) = v.as_object() else {
        return error(400, "VALIDATION_FAILED");
    };
    let known = [
        "contractVersion",
        "installationId",
        "observations",
        "coverage",
    ];
    let observations = map
        .get("observations")
        .and_then(|o| o.as_array())
        .cloned()
        .unwrap_or_default();
    let coverage = map
        .get("coverage")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let installation = s.devices[device].installation_id.clone();
    let well_formed = map.keys().all(|k| known.contains(&k.as_str()))
        && map.get("contractVersion").and_then(|c| c.as_str()) == Some("ingest/v1")
        && (installation.is_empty()
            || map.get("installationId").and_then(|i| i.as_str()) == Some(installation.as_str()))
        && observations.len() <= 500
        && coverage.len() <= 50
        && observations.len() + coverage.len() > 0;
    // A report ends after it starts, and carries a reason when (and only when) it is a gap.
    let coverage_ok = coverage.iter().all(|c| {
        let when = |k: &str| {
            c[k].as_str()
                .and_then(|t| t.parse::<jiff::Timestamp>().ok())
        };
        let ordered = when("from").zip(when("to")).is_some_and(|(a, b)| a < b);
        let reason = match c["state"].as_str() {
            Some("unavailable") => c["reason"].is_string(),
            Some("observed") => c.get("reason").is_none(),
            _ => false,
        };
        ordered && reason
    });
    if !well_formed || !coverage_ok {
        return error(400, "VALIDATION_FAILED");
    }
    s.batches
        .push((observations.len(), coverage.len(), req.body.len()));
    let server_now = now_ms() + s.clock_offset_ms;
    let when = |o: &serde_json::Value| {
        o["occurredAt"]
            .as_str()
            .and_then(|t| t.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_millisecond())
    };
    let window = s
        .ingest
        .window_start
        .as_deref()
        .and_then(|t| t.parse::<jiff::Timestamp>().ok())
        .map(|t| t.as_millisecond());
    let mut results = Vec::new();
    for (i, o) in observations.iter().enumerate() {
        let text = |k: &str| o[k].as_str().unwrap_or("").to_owned();
        let identity = (
            text("source"),
            text("externalSessionId"),
            text("externalEventId"),
        );
        // The order of the contract: authorization, fields, clock, window, then ownership, then
        // the repeat. Scripted refusals stand in for the first two and for ownership.
        let scripted = s.ingest.reject.get(&identity.2).cloned();
        let owned = scripted.as_deref() == Some("SESSION_OWNED");
        let refused = if scripted.is_some() && !owned {
            scripted.clone()
        } else if when(o).is_some_and(|t| t > server_now + WINDOW_MS) {
            Some("CLOCK_SKEW".to_owned())
        } else if window.zip(when(o)).is_some_and(|(w, t)| t < w) {
            Some("BEFORE_COLLECTION_WINDOW".to_owned())
        } else if owned {
            scripted.clone()
        } else {
            None
        };
        results.push(match refused {
            Some(code) => serde_json::json!({"index": i, "status": "rejected", "code": code}),
            None if s.identities.contains(&identity) => {
                serde_json::json!({"index": i, "status": "duplicate"})
            }
            None => {
                s.identities.insert(identity);
                s.stored.push(o.clone());
                serde_json::json!({"index": i, "status": "accepted"})
            }
        });
    }
    // Coverage is kept only for the sources whose authorization is active; the rest is ignored
    // without an error and without changing the results.
    let active: Vec<String> = s
        .sources
        .iter()
        .filter(|x| x["state"] == "active")
        .filter_map(|x| x["source"].as_str().map(str::to_owned))
        .collect();
    let kept: Vec<serde_json::Value> = coverage
        .into_iter()
        .filter(|c| active.iter().any(|a| c["source"] == a.as_str()))
        .collect();
    // For `collection` in the status: the last batch that brought something from a source (an
    // item accepted or repeated, or coverage of an active source); the furthest observed time,
    // which never goes back; and the gaps, each told after the earlier ones.
    let received = now_ms();
    for (o, r) in observations.iter().zip(&results) {
        if r["status"] != "rejected" {
            s.seen_at
                .insert(o["source"].as_str().unwrap_or("").to_owned(), received);
        }
    }
    for c in &kept {
        let ms = |k: &str| {
            c[k].as_str()
                .and_then(|t| t.parse::<jiff::Timestamp>().ok())
                .map_or(0, |t| t.as_millisecond())
        };
        let source = c["source"].as_str().unwrap_or("").to_owned();
        s.seen_at.insert(source.clone(), received);
        s.reports += 1;
        let seq = s.reports;
        if c["state"] == "observed" {
            let until = s.observed_until.entry(source.clone()).or_insert(0);
            *until = (*until).max(ms("to"));
            s.observed.push((source, ms("from"), ms("to"), seq));
        } else {
            let reason = c["reason"].as_str().unwrap_or("").to_owned();
            s.gaps.push((source, ms("from"), ms("to"), reason, seq));
        }
    }
    s.stored_coverage.extend(kept);
    if s.ingest.short_results {
        results.pop();
    }
    json(200, serde_json::json!({ "results": results }))
}

/// `collection` of the 9.4 contract: one entry per source this device has sent; the gaps are
/// the `unavailable` reports that no `observed` range told afterwards covers whole; at most 20,
/// those that start latest first.
fn collection(s: &State) -> Vec<serde_json::Value> {
    let iso = |ms: i64| jiff::Timestamp::from_millisecond(ms).unwrap().to_string();
    let mut out = Vec::new();
    for source in ["claude_code", "codex"] {
        let Some(seen) = s.seen_at.get(source) else {
            continue;
        };
        let mut open: Vec<_> =
            s.gaps
                .iter()
                .filter(|(src, from, to, _, seq)| {
                    src == source
                        && !s.observed.iter().any(|(o, a, b, later)| {
                            o == source && later > seq && a <= from && b >= to
                        })
                })
                .collect();
        open.sort_by_key(|g| std::cmp::Reverse(g.1));
        out.push(serde_json::json!({
            "source": source,
            "lastSeenAt": iso(*seen),
            "observedUntil": s.observed_until.get(source).map(|t| iso(*t)),
            "openGaps": open.iter().take(20)
                .map(|(_, from, to, reason, _)| serde_json::json!({
                    "startsAt": iso(*from), "endsAt": iso(*to), "reason": reason}))
                .collect::<Vec<_>>(),
        }));
    }
    out
}
