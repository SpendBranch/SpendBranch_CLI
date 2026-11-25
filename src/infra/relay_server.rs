//! The relay's sockets: a small HTTP/1.1 server for one path on this machine. It reads a
//! bounded head and a bounded body, asks `relay` what to do, and answers. Nothing it reads is
//! written to disk or to a log; it hands the filtered payload to `forward` and forgets it.
use super::clock;
use crate::relay::{self, Head, RateLimit};
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_HEAD: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_CONNECTIONS: usize = 32;
const READ_WAIT: Duration = Duration::from_secs(5);
/// A client that has not sent its whole request by now is cut off.
const REQUEST_WAIT: Duration = Duration::from_secs(10);
/// The headers come at once from an exporter; whoever trickles them gives the slot back soon.
const HEAD_WAIT: Duration = Duration::from_secs(2);
/// After an early answer, how much of the unread body is taken in, and for how long, so the
/// close is a clean end of stream and not a reset that would lose the answer.
const DRAIN_MAX: u64 = 256 * 1024;
const DRAIN_WAIT: Duration = Duration::from_millis(500);

/// What happened to a batch, for the screen.
pub enum Event {
    /// The batch went on; `malformed` declared values were left out of it (a count, never a value).
    Forwarded { malformed: usize },
    /// The batch was lost: why, in words that repeat nothing the agent sent.
    Dropped(String),
}

/// Sends a filtered payload on. `Ok` carries how many records the server did not take.
pub type Forward<'a> = &'a (dyn Fn(&Value) -> Result<u64, String> + Sync);

/// Listens on this machine only: the address is fixed, never taken from the command line.
pub fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port))
}

/// Ends the wait of `serve` for the next connection, once `stop` is set.
pub fn wake(port: u16) {
    let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, port));
}

/// Serves until `stop` is set; after setting it, connect to the listener once, so the wait for
/// the next connection ends. Connections are handled on their own threads, at most 32 at once.
/// Only a request that carries `token` gets past the door.
pub fn serve(
    listener: &TcpListener,
    stop: &AtomicBool,
    token: &str,
    forward: Forward,
    events: &Sender<Event>,
) -> io::Result<()> {
    let limit = Mutex::new(RateLimit::default());
    let active = AtomicUsize::new(0);
    thread::scope(|scope| {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, peer)) => {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if !same_user(&stream, peer) {
                        continue; // another account: closed before a byte is read
                    }
                    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        active.fetch_sub(1, Ordering::SeqCst);
                        let _ = reply(&stream, 503);
                        continue;
                    }
                    let (limit, active, events) = (&limit, &active, events.clone());
                    scope.spawn(move || {
                        let _ = handle(stream, peer, token, limit, forward, &events);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // A connection that died before it was accepted is not the listener's problem.
                Err(e) if e.kind() == io::ErrorKind::ConnectionAborted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })
}

/// Whether the client runs as this user. Linux says so in `/proc/net/tcp`, macOS through
/// `lsof`; where neither exists (Windows) every local client passes and the token is the only
/// door. What cannot be found out is a no, except a system with no such table or tool at all.
// ponytail: Windows (GetExtendedTcpTable) needs FFI, which this crate does not allow.
pub fn same_user(stream: &TcpStream, peer: SocketAddr) -> bool {
    #[cfg(target_os = "macos")]
    {
        same_user_by_lsof(stream, peer)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (SocketAddr::V4(peer), Ok(SocketAddr::V4(us))) = (peer, stream.local_addr()) else {
            return false;
        };
        match std::fs::read_to_string("/proc/net/tcp") {
            Ok(table) => {
                let me = super::private_fs::my_uid().ok();
                let end = |a: std::net::SocketAddrV4| (a.ip().octets(), a.port());
                me.is_some() && relay::peer_uid(&table, end(peer), end(us)) == me
            }
            Err(e) => e.kind() == io::ErrorKind::NotFound,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (stream, peer);
        true
    }
}

/// macOS keeps no table to read, and the socket call that names a peer works only for local
/// (Unix) sockets. `lsof`, run by its full path with no shell, lists this user's sockets on the
/// client's port: the client's end must be among them, held by a process of this user. Another
/// user's sockets are not listed to us, so they never pass. Bounded by `LSOF_WAIT`.
#[cfg(target_os = "macos")]
fn same_user_by_lsof(stream: &TcpStream, peer: SocketAddr) -> bool {
    use std::process::{Command, Stdio};
    const LSOF: &str = "/usr/sbin/lsof";
    const LSOF_WAIT: Duration = Duration::from_secs(2);
    let (SocketAddr::V4(peer), Ok(SocketAddr::V4(us))) = (peer, stream.local_addr()) else {
        return false;
    };
    let Ok(me) = super::private_fs::my_uid() else {
        return false;
    };
    let spawned = Command::new(LSOF)
        .args(["-n", "-P", "-Fpun"])
        .arg(format!("-iTCP@{}:{}", peer.ip(), peer.port()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        // No such tool on this system: like Linux without its table, the token is the door.
        Err(e) => return e.kind() == io::ErrorKind::NotFound,
    };
    let deadline = Instant::now() + LSOF_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
    let mut listed = String::new();
    let read = child
        .stdout
        .take()
        .map(|out| out.take(64 * 1024).read_to_string(&mut listed));
    let end = |a: std::net::SocketAddrV4| (a.ip().octets(), a.port());
    matches!(read, Some(Ok(_))) && relay::lsof_peer_uid(&listed, end(peer), end(us)) == Some(me)
}

fn reply(mut stream: &TcpStream, status: u16) -> io::Result<()> {
    let body = if status == 200 {
        "{}".to_owned()
    } else {
        format!("{{\"error\":\"{}\"}}", relay::reason(status))
    };
    reply_with(stream, status, &body)?;
    stream.flush()
}

fn reply_with(mut stream: &TcpStream, status: u16, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        relay::reason(status),
        body.len()
    )
}

/// Answers before the body was read, then lets the client finish sending (up to `DRAIN_MAX` of
/// what it announced, within `DRAIN_WAIT`) and discards it. Closing with unread bytes, or with
/// bytes still on their way, makes the system reset the connection, and the client loses the
/// status (a refused batch would look like a network failure and be sent again at once).
fn reply_early(
    stream: &TcpStream,
    status: u16,
    announced: Option<u64>,
    had: usize,
) -> io::Result<()> {
    let sent = reply(stream, status);
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let left = announced
        .unwrap_or(DRAIN_MAX)
        .min(DRAIN_MAX)
        .saturating_sub(had as u64);
    let deadline = Instant::now() + DRAIN_WAIT;
    let mut taken = 0u64;
    let mut chunk = [0u8; 8192];
    while taken < left {
        let Some(n) = read_by(stream, &mut chunk, deadline) else {
            break;
        };
        taken += n as u64;
    }
    sent
}

/// One read that never waits past `deadline`: a client that trickles bytes cannot hold a slot
/// longer than the request's whole allowance. `None` when gone, too slow or out of time.
fn read_by(mut stream: &TcpStream, buf: &mut [u8], deadline: Instant) -> Option<usize> {
    let left = deadline.checked_duration_since(Instant::now())?;
    stream
        .set_read_timeout(Some(left.min(READ_WAIT).max(Duration::from_millis(1))))
        .ok()?;
    match stream.read(buf) {
        Ok(0) | Err(_) => None,
        Ok(n) => Some(n),
    }
}

/// The request line and headers as text, and the body bytes that came with them.
fn read_head(stream: &TcpStream, deadline: Instant) -> Result<(String, Vec<u8>), u16> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8(buf[..end].to_vec()).map_err(|_| 400u16)?;
            return Ok((head, buf[end + 4..].to_vec()));
        }
        if buf.len() > MAX_HEAD {
            return Err(400);
        }
        // Gone, too slow or out of time: nothing to answer.
        let n = read_by(stream, &mut chunk, deadline).ok_or(0u16)?;
        buf.extend_from_slice(&chunk[..n]);
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    /// `Err` for what no sane client sends: a malformed line, too many headers, or a header
    /// that the checks depend on given twice (the way requests are smuggled past a filter).
    fn parse(head: &str) -> Option<Self> {
        let mut lines = head.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, path, version) = (first.next()?, first.next()?, first.next()?);
        if first.next().is_some() || !version.starts_with("HTTP/1.") {
            return None;
        }
        let mut headers = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':')?;
            if name.is_empty() || name.contains(' ') || headers.len() >= MAX_HEADERS {
                return None;
            }
            headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
        }
        for name in [
            "host",
            "origin",
            relay::TOKEN_HEADER,
            "content-type",
            "content-encoding",
            "content-length",
            "transfer-encoding",
        ] {
            if headers.iter().filter(|(n, _)| n == name).count() > 1 {
                return None;
            }
        }
        Some(Self {
            method: method.to_owned(),
            path: path.to_owned(),
            headers,
        })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// `Err` for a `Content-Length` that is not plain digits; `Ok(None)` when there is none.
fn content_length(request: &Request) -> Result<Option<u64>, ()> {
    let Some(text) = request.get("content-length") else {
        return Ok(None);
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    text.parse().map(Some).map_err(|_| ())
}

/// The announced body: what came with the head, then the rest. `None` if the client is gone
/// or too slow.
fn read_body(
    stream: &TcpStream,
    mut body: Vec<u8>,
    wanted: usize,
    deadline: Instant,
) -> Option<Vec<u8>> {
    body.truncate(wanted); // bytes sent after the body (pipelining) are not ours
    let mut chunk = [0u8; 8192];
    while body.len() < wanted {
        let room = (wanted - body.len()).min(chunk.len());
        let n = read_by(stream, &mut chunk[..room], deadline)?;
        body.extend_from_slice(&chunk[..n]);
    }
    Some(body)
}

fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    token: &str,
    limit: &Mutex<RateLimit>,
    forward: Forward,
    events: &Sender<Event>,
) -> io::Result<()> {
    stream.set_write_timeout(Some(READ_WAIT))?;
    let started = Instant::now();
    let deadline = started + REQUEST_WAIT;
    let (head, body) = match read_head(&stream, started + HEAD_WAIT) {
        Ok(h) => h,
        Err(0) => return Ok(()),
        Err(status) => return reply_early(&stream, status, None, 0),
    };
    let Some(request) = Request::parse(&head) else {
        return reply_early(&stream, 400, None, body.len());
    };
    let announced = content_length(&request).ok().flatten();
    // The token is checked here, before the rate limit counts anything.
    let wanted = match screen(&request, peer, token) {
        Ok(n) => n,
        Err(status) => return reply_early(&stream, status, announced, body.len()),
    };
    if !limit
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .allow(clock::now_ms())
    {
        return reply_early(&stream, 429, announced, body.len());
    }
    let Some(body) = read_body(&stream, body, wanted, deadline) else {
        return Ok(()); // gone or too slow: nothing to answer
    };
    let prepared = match relay::prepare(&body) {
        Ok(p) => p,
        Err(status) => return reply(&stream, status),
    };
    drop(body);
    if prepared.kept == 0 {
        return reply_with(&stream, 200, &relay::answer(prepared.refused));
    }
    match forward(&prepared.payload) {
        Ok(by_server) => {
            let _ = events.send(Event::Forwarded {
                malformed: prepared.malformed,
            });
            reply_with(&stream, 200, &relay::answer(prepared.refused + by_server))
        }
        Err(why) => {
            let _ = events.send(Event::Dropped(why));
            reply(&stream, 502)
        }
    }
}

/// The request's head against the relay's table of answers: the length of the body to read, or
/// the status to answer with.
fn screen(request: &Request, peer: SocketAddr, token: &str) -> Result<usize, u16> {
    let length = content_length(request);
    let checked = relay::check(
        &Head {
            peer_is_loopback: peer.ip().to_canonical().is_loopback(),
            method: &request.method,
            path: &request.path,
            host: request.get("host"),
            origin: request.get("origin"),
            token: request.get(relay::TOKEN_HEADER),
            content_type: request.get("content-type"),
            content_encoding: request.get("content-encoding"),
            transfer_encoding: request.get("transfer-encoding"),
            content_length: length.unwrap_or(None),
        },
        token,
    );
    // A bad length is a 400, but only for a request that got past the door.
    match (checked, length) {
        (Err(403), _) => Err(403),
        (_, Err(())) => Err(400),
        (checked, _) => checked,
    }
}
