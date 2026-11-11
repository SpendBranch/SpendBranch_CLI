//! A real HTTP server on a local port that behaves like the API's device routes: it pairs
//! devices and checks request signatures in the order the spec lists them.
//!
//! It deliberately does not use the crate's own `auth` module. It builds the signed text
//! by hand from the spec, so a mistake in the client's code is not repeated here.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
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
}

#[derive(Default)]
pub struct State {
    pub devices: HashMap<String, Device>,
    /// (device, nonce): what the replay check remembers.
    nonces: Vec<(String, String)>,
    /// How far this server's clock is from the real one.
    clock_offset_ms: i64,
    pub seen: Vec<Seen>,
    /// Where `/redirect` sends the caller.
    pub redirect_to: String,
    pairing: Option<(String, [u8; 32], bool)>,
    next_device: u32,
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
        let state = Arc::new(Mutex::new(State::default()));
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
        let (_, key, approved) = s.pairing.as_mut().expect("no pairing started");
        let key = VerifyingKey::from_bytes(key).unwrap();
        *approved = true;
        s.devices.insert(
            id.clone(),
            Device {
                key,
                active: true,
                revoked_reason: None,
            },
        );
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
            },
        );
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
            Ok(_) => json(200, serde_json::json!({"organizationName": "Acme"})),
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
            Ok(_) => json(200, serde_json::json!({"results": []})),
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
    s.pairing = Some(("dc-abcdefghijklmnopqrstuvwxyz".into(), key, false));
    json(
        200,
        serde_json::json!({
            "deviceCode": "dc-abcdefghijklmnopqrstuvwxyz",
            "userCode": "BCDF-GHJK",
            "verificationUri": "http://127.0.0.1/devices/pair",
            "verificationUriComplete": "http://127.0.0.1/devices/pair?code=BCDF-GHJK",
            "expiresIn": 600,
            "interval": 5
        }),
    )
}

fn poll(s: &mut State, body: &[u8]) -> Reply {
    let code = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["deviceCode"].as_str().map(str::to_owned));
    let status = |st: &str| json(200, serde_json::json!({ "status": st }));
    match (&s.pairing, code) {
        (Some((expected, _, approved)), Some(code)) if *expected == code => {
            if *approved {
                let id = format!("dev-{}", s.next_device);
                json(
                    200,
                    serde_json::json!({
                        "status": "approved", "deviceId": id,
                        "expiresAt": "2027-02-08T00:00:00Z",
                        "organizationId": "org_1", "organizationName": "Acme Inc."
                    }),
                )
            } else {
                status("authorization_pending")
            }
        }
        _ => status("expired_token"),
    }
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
    // 4. the nonce was not seen for this device: a repeat with a valid signature revokes
    let pair = (id.to_owned(), nonce.to_owned());
    if s.nonces.contains(&pair) {
        let d = s.devices.get_mut(id).unwrap();
        (d.active, d.revoked_reason) = (false, Some("auto:replay".into()));
        return Err(unauth());
    }
    s.nonces.push(pair);
    Ok(id.to_owned())
}
