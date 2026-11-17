//! The rules of the local OTLP relay: which requests it answers and how. Pure; the sockets
//! are in `infra::relay_server`.
//!
//! The relay trusts nothing that reaches it: only this machine, only a `Host` that names this
//! machine (a web page cannot make the browser lie about it: DNS rebinding), no `Origin` at
//! all (a browser sends one, an exporter does not), the token `otel` wrote for the agent (any
//! other process, another account included, gets nothing signed), small bodies, and a ceiling
//! on batches.
use crate::sources::otlp;
use serde_json::Value;

pub const PATH: &str = "/v1/logs";
/// The header that carries the token. Plain hex in a plain header: no OTLP header encoding to
/// get wrong (`OTEL_EXPORTER_OTLP_HEADERS` is `name=value`).
pub const TOKEN_HEADER: &str = "x-spendbranch-token";
/// The ports `otel` picks from: under every system's ephemeral range (Linux 32768+, Windows and
/// macOS 49152+), so no outgoing connection holds one by chance, and never the well-known 4318.
pub const PORTS: std::ops::RangeInclusive<u16> = 20_000..=32_767;
pub const MAX_BODY: u64 = 1024 * 1024;
/// The same ceiling the server keeps for these routes: more would only come back refused.
pub const MAX_PER_MINUTE: u32 = 60;
const WINDOW_MS: i64 = 60_000;

/// What the server read of the request line and the headers.
pub struct Head<'a> {
    pub peer_is_loopback: bool,
    pub method: &'a str,
    pub path: &'a str,
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub token: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub content_encoding: Option<&'a str>,
    pub transfer_encoding: Option<&'a str>,
    pub content_length: Option<u64>,
}

fn host_is_local(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let (name, port) = match host.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((inside, "")) => (format!("[{inside}]"), None),
            Some((inside, after)) => match after.strip_prefix(':') {
                Some(port) => (format!("[{inside}]"), Some(port)),
                None => return false, // `[::1]x`
            },
            None => return false,
        },
        None => match host.split_once(':') {
            Some((n, p)) => (n.to_owned(), Some(p)),
            None => (host.clone(), None),
        },
    };
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    port_ok && matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

/// The token and port `otel` hands the agent, from 34 random bytes: 32 for the token, 2 for
/// the port.
pub fn new_endpoint(random: [u8; 34]) -> (String, u16) {
    let token = random[..32].iter().map(|b| format!("{b:02x}")).collect();
    let span = PORTS.end() - PORTS.start() + 1;
    (
        token,
        PORTS.start() + u16::from_le_bytes([random[32], random[33]]) % span,
    )
}

/// Who owns the client's end of a loopback connection, read from Linux's `/proc/net/tcp`: the
/// line whose local address is the client (`peer`) and whose remote address is the relay (`us`).
/// Addresses there are the raw network-order word printed in host order, ports in hex.
pub fn peer_uid(
    table: &str,
    peer: std::net::SocketAddrV4,
    us: std::net::SocketAddrV4,
) -> Option<u32> {
    let hex = |a: std::net::SocketAddrV4| {
        format!(
            "{:08X}:{:04X}",
            u32::from_ne_bytes(a.ip().octets()),
            a.port()
        )
    };
    let (peer, us) = (hex(peer), hex(us));
    table.lines().skip(1).find_map(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        (f.get(1) == Some(&peer.as_str()) && f.get(2) == Some(&us.as_str()))
            .then(|| f.get(7)?.parse().ok())?
    })
}

/// Equal in time that does not depend on where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

/// The first failing check, in the order of the plan's table, as a status code. `Ok` is the
/// length of the body to read. Everything that answers 403 comes before any other answer, so
/// nothing else is learned (or counted) without the token.
pub fn check(head: &Head, token: &str) -> Result<usize, u16> {
    if !head.peer_is_loopback
        || head.origin.is_some()
        || !head.host.is_some_and(host_is_local)
        || !head.token.is_some_and(|t| same(t, token))
    {
        return Err(403);
    }
    if head.method != "POST" {
        return Err(405);
    }
    if head.path != PATH {
        return Err(404);
    }
    let json = head.content_type.is_some_and(|t| {
        t.trim_start()
            .to_ascii_lowercase()
            .starts_with("application/json")
    });
    if !json || head.content_encoding.is_some() {
        return Err(415);
    }
    // Only a body whose length is announced is read: no chunked bodies.
    let (None, Some(length)) = (head.transfer_encoding, head.content_length) else {
        return Err(411);
    };
    if length > MAX_BODY {
        return Err(413);
    }
    usize::try_from(length).map_err(|_| 413)
}

/// At most `MAX_PER_MINUTE` batches a minute, in fixed windows.
#[derive(Default)]
pub struct RateLimit {
    start_ms: i64,
    count: u32,
}

impl RateLimit {
    pub fn allow(&mut self, now_ms: i64) -> bool {
        if now_ms - self.start_ms >= WINDOW_MS || now_ms < self.start_ms {
            (self.start_ms, self.count) = (now_ms, 0);
        }
        self.count += 1;
        self.count <= MAX_PER_MINUTE
    }
}

/// A body that is JSON, filtered: what to forward and how many records stayed behind.
pub struct Prepared {
    pub payload: Value,
    pub kept: usize,
    pub refused: u64,
}

pub fn prepare(body: &[u8]) -> Result<Prepared, u16> {
    let value: Value = serde_json::from_slice(body).map_err(|_| 400u16)?;
    let f = otlp::lenient(&value);
    Ok(Prepared {
        payload: f.payload,
        kept: f.kept,
        refused: u64::try_from(f.dropped + f.ignored).unwrap_or(u64::MAX),
    })
}

/// The `200` body: `{}`, or how many records were not taken.
pub fn answer(refused: u64) -> String {
    if refused == 0 {
        "{}".into()
    } else {
        format!("{{\"partialSuccess\":{{\"rejectedLogRecords\":{refused}}}}}")
    }
}

/// How many records the server said it did not take (`{}` when it took all).
pub fn refused_by_server(body: &Value) -> u64 {
    body.pointer("/partialSuccess/rejectedLogRecords")
        .and_then(|n| n.as_u64().or_else(|| n.as_str()?.parse().ok()))
        .unwrap_or(0)
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        _ => "Service Unavailable",
    }
}
