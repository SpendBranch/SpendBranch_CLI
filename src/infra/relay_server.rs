//! The relay's sockets: a small HTTP/1.1 server for one path on this machine. It reads a
//! bounded head and a bounded body, asks `relay` what to do, and answers. Nothing it reads is
//! written to disk or to a log; it hands the filtered payload to `forward` and forgets it.
use super::clock::{Clock, SystemClock};
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

/// What happened to a batch, for the screen.
pub enum Event {
    Forwarded,
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
pub fn serve(
    listener: &TcpListener,
    stop: &AtomicBool,
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
                    if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        active.fetch_sub(1, Ordering::SeqCst);
                        let _ = reply(&stream, 503);
                        continue;
                    }
                    let (limit, active, events) = (&limit, &active, events.clone());
                    scope.spawn(move || {
                        let _ = handle(stream, peer, limit, forward, &events);
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

/// The request line and headers as text, and the body bytes that came with them.
fn read_head(mut stream: &TcpStream, deadline: Instant) -> Result<(String, Vec<u8>), u16> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8(buf[..end].to_vec()).map_err(|_| 400u16)?;
            return Ok((head, buf[end + 4..].to_vec()));
        }
        if buf.len() > MAX_HEAD || Instant::now() > deadline {
            return Err(400);
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(0), // gone or too slow: nothing to answer
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
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
    mut stream: &TcpStream,
    mut body: Vec<u8>,
    wanted: usize,
    deadline: Instant,
) -> Option<Vec<u8>> {
    body.truncate(wanted); // bytes sent after the body (pipelining) are not ours
    let mut chunk = [0u8; 8192];
    while body.len() < wanted {
        if Instant::now() > deadline {
            return None;
        }
        let room = (wanted - body.len()).min(chunk.len());
        match stream.read(&mut chunk[..room]) {
            Ok(0) | Err(_) => return None,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    Some(body)
}

fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    limit: &Mutex<RateLimit>,
    forward: Forward,
    events: &Sender<Event>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_WAIT))?;
    stream.set_write_timeout(Some(READ_WAIT))?;
    let deadline = Instant::now() + REQUEST_WAIT;
    let (head, body) = match read_head(&stream, deadline) {
        Ok(h) => h,
        Err(0) => return Ok(()),
        Err(status) => return reply(&stream, status),
    };
    let Some(request) = Request::parse(&head) else {
        return reply(&stream, 400);
    };
    let Ok(content_length) = content_length(&request) else {
        return reply(&stream, 400);
    };
    let checked = relay::check(&Head {
        peer_is_loopback: peer.ip().to_canonical().is_loopback(),
        method: &request.method,
        path: &request.path,
        host: request.get("host"),
        origin: request.get("origin"),
        content_type: request.get("content-type"),
        content_encoding: request.get("content-encoding"),
        transfer_encoding: request.get("transfer-encoding"),
        content_length,
    });
    let wanted = match checked {
        Ok(n) => n,
        Err(status) => return reply(&stream, status),
    };
    if !limit
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .allow(SystemClock.now_ms())
    {
        return reply(&stream, 429);
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
            let _ = events.send(Event::Forwarded);
            reply_with(&stream, 200, &relay::answer(prepared.refused + by_server))
        }
        Err(why) => {
            let _ = events.send(Event::Dropped(why));
            reply(&stream, 502)
        }
    }
}
