//! The relay over a real socket, and the real program forwarding, signed, to the fake server.
use crate::rig;

use rig::{Rig, authorize, free_port, payload, record};
use serde_json::{Value, json};
use spendbranch::infra::relay_server::{self, Event};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

// --- the server over a real socket ----------------------------------------------------

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const AUTH: &str =
    "X-SpendBranch-Token: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\r\n";

struct Relay {
    port: u16,
    stop: Arc<AtomicBool>,
    sent: Arc<Mutex<Vec<Value>>>,
    events: mpsc::Receiver<Event>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Relay {
    /// `answer` is what the forward step does with a payload.
    fn start(answer: impl Fn(&Value) -> Result<u64, String> + Sync + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (stop, sent) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(Vec::new())),
        );
        let (tx, events) = mpsc::channel();
        let (stop2, sent2) = (stop.clone(), sent.clone());
        let thread = thread::spawn(move || {
            let forward = move |p: &Value| {
                sent2.lock().unwrap().push(p.clone());
                answer(p)
            };
            relay_server::serve(&listener, &stop2, TOKEN, &forward, &tx).unwrap();
        });
        Self {
            port,
            stop,
            sent,
            events,
            thread: Some(thread),
        }
    }

    /// Sends raw bytes and returns (status, body) of the answer.
    fn raw(&self, request: &[u8]) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = stream.write_all(request);
        let mut text = Vec::new();
        let _ = stream.read_to_end(&mut text);
        let text = String::from_utf8_lossy(&text).into_owned();
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (
            status,
            text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned(),
        )
    }

    /// A post with the token, as the agent sends it.
    fn post(&self, headers: &str, body: &str) -> (u16, String) {
        self.post_as(&format!("{AUTH}{headers}"), body)
    }

    fn post_as(&self, headers: &str, body: &str) -> (u16, String) {
        let req = format!(
            "POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            self.port,
            body.len()
        );
        self.raw(req.as_bytes())
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // ends the wait for a connection
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn sample() -> String {
    payload(vec![record(
        "api_request",
        json!([{"key": "session.id", "value": {"stringValue": "s"}}, {"key": "prompt", "value": {"stringValue": "SECRET-PROMPT"}}]),
    )])
    .to_string()
}

#[test]
fn a_good_batch_is_filtered_forwarded_and_answered_200() {
    let relay = Relay::start(|_| Ok(0));
    let (status, body) = relay.post("", &sample());
    assert_eq!((status, body.as_str()), (200, "{}"));
    let sent = relay.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].to_string().contains("SECRET"), "{}", sent[0]);
    assert!(matches!(
        relay.events.try_recv(),
        Ok(Event::Forwarded { malformed: 0 })
    ));
}

#[test]
fn a_forwarded_batch_says_how_many_declared_values_were_left_out() {
    let relay = Relay::start(|_| Ok(0));
    let body = payload(vec![record(
        "api_request",
        json!([
            {"key": "session.id", "value": {"stringValue": "s"}},
            {"key": "input_tokens", "value": {"intValue": "-5"}},
            {"key": "model", "value": {"stringValue": "not a model"}},
        ]),
    )]);
    let (status, _) = relay.post("", &body.to_string());
    assert_eq!(status, 200);
    assert!(matches!(
        relay.events.try_recv(),
        Ok(Event::Forwarded { malformed: 2 })
    ));
}

#[test]
fn the_malformed_count_repeats_nothing_the_agent_sent_and_fits_80_columns() {
    let line = spendbranch::cli::messages::relay_malformed(1_000_000);
    assert!(line.chars().count() <= 80, "{line}");
    assert!(line.contains("1000000"), "{line}");
}

#[test]
fn records_that_are_not_an_api_request_are_counted_and_not_forwarded_when_none_is_left() {
    let relay = Relay::start(|_| Ok(0));
    let only_other = payload(vec![record("user_prompt", json!([]))]).to_string();
    let (status, body) = relay.post("", &only_other);
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"partialSuccess":{"rejectedLogRecords":1}}"#);
    assert!(
        relay.sent.lock().unwrap().is_empty(),
        "nothing to forward, no connection to the server"
    );
    let mixed = payload(vec![
        record("user_prompt", json!([])),
        record("api_request", json!([])),
    ])
    .to_string();
    let (_, body) = relay.post("", &mixed);
    assert_eq!(body, r#"{"partialSuccess":{"rejectedLogRecords":1}}"#);
    assert_eq!(relay.sent.lock().unwrap().len(), 1);
}

#[test]
fn what_the_server_refuses_is_added_to_the_answer() {
    let relay = Relay::start(|_| Ok(2));
    let (_, body) = relay.post("", &sample());
    assert_eq!(body, r#"{"partialSuccess":{"rejectedLogRecords":2}}"#);
}

#[test]
fn a_failed_forward_is_a_502_and_nothing_is_kept() {
    let relay = Relay::start(|_| Err("Could not reach the server.".into()));
    let (status, body) = relay.post("", &sample());
    assert_eq!(status, 502);
    assert!(!body.contains("SECRET"));
    match relay.events.try_recv() {
        Ok(Event::Dropped(why)) => assert_eq!(why, "Could not reach the server."),
        _ => panic!("the drop was not reported"),
    }
}

#[test]
fn bad_requests_get_their_status_and_never_reach_the_forward_step() {
    let relay = Relay::start(|_| Ok(0));
    let p = relay.port;
    let line = |l: &str| {
        format!(
            "{l}\r\nHost: 127.0.0.1:{p}\r\n{AUTH}Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
        )
    };
    let answers = [
        ("GET", relay.raw(line("GET /v1/logs HTTP/1.1").as_bytes()).0, 405),
        ("other path", relay.raw(line("POST /v1/metrics HTTP/1.1").as_bytes()).0, 404),
        ("not HTTP", relay.raw(b"hello\r\n\r\n").0, 400),
        ("bad version", relay.raw(line("POST /v1/logs SPDY/3").as_bytes()).0, 400),
        ("rebinding", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: evil.example\r\n{AUTH}Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes()).0, 403),
        ("origin", relay.post("Origin: http://localhost\r\n", "{}").0, 403),
        ("gzip", relay.post("Content-Encoding: gzip\r\n", "{}").0, 415),
        ("bad json", relay.post("", "{nope").0, 400),
        ("empty", relay.post("", "").0, 400),
        ("chunked", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\n{AUTH}Content-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n").as_bytes()).0, 411),
        ("huge announced", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\n{AUTH}Content-Type: application/json\r\nContent-Length: 99999999\r\n\r\n").as_bytes()).0, 413),
        ("two lengths", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\n{AUTH}Content-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 5\r\n\r\n{{}}").as_bytes()).0, 400),
        ("signed length", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\n{AUTH}Content-Type: application/json\r\nContent-Length: +2\r\n\r\n{{}}").as_bytes()).0, 400),
        ("endless head", relay.raw(&[b"POST /v1/logs HTTP/1.1\r\nX: ".as_slice(), &vec![b'a'; 20_000]].concat()).0, 400),
        ("many headers", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\n{}\r\n", "A: b\r\n".repeat(100)).as_bytes()).0, 400),
    ];
    for (name, got, want) in answers {
        assert_eq!(got, want, "{name}");
    }
    assert!(relay.sent.lock().unwrap().is_empty());
}

#[test]
fn bytes_after_the_body_are_not_read_as_part_of_it() {
    let relay = Relay::start(|_| Ok(0));
    let body = sample();
    let (status, _) = relay.post("", &format!("{body}GARBAGE-AFTER-THE-BODY"));
    assert_eq!(
        status, 400,
        "the announced length is shorter than what was sent: the JSON is cut"
    );
    // ...and exactly the announced length is read when it is right.
    let req = format!(
        "POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{AUTH}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}TRAILING",
        relay.port,
        body.len()
    );
    assert_eq!(relay.raw(req.as_bytes()).0, 200);
}

#[test]
fn the_61st_batch_in_a_minute_is_refused() {
    let relay = Relay::start(|_| Ok(0));
    let mut last = 0;
    for _ in 0..61 {
        last = relay.post("", "{}").0;
        if last == 429 {
            break;
        }
    }
    assert_eq!(last, 429);
}

#[test]
fn without_the_token_nothing_is_signed_and_nothing_is_counted() {
    let relay = Relay::start(|_| Ok(0));
    for headers in [
        "",
        "X-SpendBranch-Token: nope\r\n",
        "Authorization: Bearer 0123\r\n",
    ] {
        assert_eq!(relay.post_as(headers, &sample()).0, 403, "{headers:?}");
    }
    // Two tokens, one of them right: refused, never "the first one wins".
    assert_eq!(relay.post(AUTH, &sample()).0, 400);
    // A flood without the token does not use up the agent's 60 a minute.
    let p = relay.port;
    for _ in 0..80 {
        let head = format!(
            "POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nContent-Type: application/json\r\nContent-Length: 1048576\r\n\r\n"
        );
        assert_eq!(relay.raw(head.as_bytes()).0, 403);
    }
    assert!(relay.sent.lock().unwrap().is_empty());
    assert_eq!(relay.post("", &sample()).0, 200);
}

#[test]
fn a_client_that_trickles_is_cut_off_when_its_time_is_up() {
    let relay = Relay::start(|_| Ok(0));
    let mut slow = TcpStream::connect(("127.0.0.1", relay.port)).unwrap();
    let mut writer = slow.try_clone().unwrap();
    // A byte every 4 s: each read is within its own wait, but the request never ends.
    thread::spawn(move || {
        while writer.write_all(b"P").is_ok() {
            thread::sleep(Duration::from_secs(4));
        }
    });
    slow.set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let started = Instant::now();
    let _ = slow.read(&mut [0u8; 64]); // returns when the relay answers or closes
    let closed = started.elapsed();
    assert!(
        closed < Duration::from_millis(11_500),
        "cut off after {closed:?}"
    );
}

#[test]
fn a_client_that_trickles_its_headers_gives_its_slot_back_within_seconds() {
    let relay = Relay::start(|_| Ok(0));
    let mut slow = TcpStream::connect(("127.0.0.1", relay.port)).unwrap();
    let mut writer = slow.try_clone().unwrap();
    thread::spawn(move || {
        while writer.write_all(b"X").is_ok() {
            thread::sleep(Duration::from_millis(500));
        }
    });
    slow.set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let started = Instant::now();
    let _ = slow.read(&mut [0u8; 64]);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
#[cfg(target_os = "linux")]
fn another_account_is_closed_on_even_with_the_right_token() {
    // A second account: this user's subordinate ids, through a user namespace. Skipped where
    // the machine has none (no /etc/subuid entry or no newuidmap).
    let other = |port: u16, body: &str| {
        let py = format!(
            "import socket;b={body:?}.encode();s=socket.create_connection(('127.0.0.1',{port}));\
             s.sendall(b'POST /v1/logs HTTP/1.1\\r\\nHost: localhost\\r\\n{AUTH}Content-Type: application/json\\r\\nContent-Length: '+str(len(b)).encode()+b'\\r\\n\\r\\n'+b);\
             s.settimeout(10);print(repr(s.recv(4096)))",
            AUTH = AUTH.trim_end().replace("\r", "")
        );
        Command::new("unshare")
            .args(["--user", "--map-auto", "--setuid", "1", "--setgid", "1"])
            .args(["python3", "-I", "-c", &py])
            .output()
    };
    let probe = Command::new("unshare")
        .args(["--user", "--map-auto", "--setuid", "1", "true"])
        .status();
    if !probe.is_ok_and(|s| s.success()) {
        return;
    }
    let relay = Relay::start(|_| Ok(0));
    let out = other(relay.port, &sample()).unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        !said.contains("HTTP/1.1"),
        "answered another account: {said}"
    );
    assert!(relay.sent.lock().unwrap().is_empty());
    assert_eq!(
        relay.post("", &sample()).0,
        200,
        "this account still gets in"
    );
}

#[test]
fn a_client_that_stalls_does_not_block_the_others() {
    let relay = Relay::start(|_| Ok(0));
    let mut slow = TcpStream::connect(("127.0.0.1", relay.port)).unwrap();
    slow.write_all(b"POST /v1/logs HTTP/1.1\r\nHost: 1")
        .unwrap(); // and nothing more
    let started = Instant::now();
    assert_eq!(relay.post("", &sample()).0, 200);
    assert!(started.elapsed() < Duration::from_secs(2));
}

// --- the real program -----------------------------------------------------------------

struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_program(rig: &Rig, port: u16) -> Child {
    let child: std::process::Child = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
        .args(["relay", "--port", &port.to_string()])
        .env("SPENDBRANCH_HOME", rig.dir.path())
        .env("SPENDBRANCH_KEY_STORE", "file")
        .env("HOME", rig.dir.path())
        .env("USERPROFILE", rig.dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let child = Child(child);
    let until = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < until, "the relay did not start");
        thread::sleep(Duration::from_millis(50));
    }
    child
}

/// What `otel` handed the agent: (port, token).
fn agent_endpoint(rig: &Rig) -> (u16, String) {
    assert_eq!(rig.run(&["otel", "claude-code"]).code, 0);
    let store = rig.store();
    let port = store
        .meta_get("relay_port")
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    (port, store.meta_get("relay_token").unwrap().unwrap())
}

fn post_to(port: u16, token: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(stream, "POST /v1/logs HTTP/1.1\r\nHost: localhost:{port}\r\nX-SpendBranch-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut text = String::new();
    let _ = stream.read_to_string(&mut text);
    (
        text.split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned(),
    )
}

fn files_in(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files_in(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn the_program_forwards_signed_what_passed_the_filter_and_keeps_nothing() {
    let rig = Rig::new();
    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let (_, token) = agent_endpoint(&rig);
    let port = free_port();
    let child = start_program(&rig, port);

    let (status, body) = post_to(port, &token, &sample());
    assert_eq!((status, body.as_str()), (200, "{}"));
    // The fake server only answers 200 to a request whose signature checks out.
    let received = rig.server.state.lock().unwrap().otlp.received.clone();
    assert_eq!(received.len(), 1);
    let text = received[0].1.to_string();
    assert!(
        text.contains("\"session.id\"") && !text.contains("SECRET"),
        "{text}"
    );
    let seen = rig.server.seen();
    let sent = seen
        .iter()
        .find(|s| s.path == "/api/ingest/otlp/v1/logs")
        .unwrap();
    assert_eq!(
        sent.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert!(sent.headers.contains_key("x-sb-signature"));

    // A server that answers with an error: 502 to the agent, and the drop is announced.
    rig.server
        .configure(|s| s.otlp.fail_next.push_back((503, "UNAVAILABLE".into())));
    assert_eq!(post_to(port, &token, &sample()).0, 502);
    assert_eq!(
        post_to(port, "forged", &sample()).0,
        403,
        "another process, no token"
    );

    let mut child = child;
    child.0.kill().unwrap();
    let mut out = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let mut err = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(
        out.contains(&format!(
            "Relay listening on http://127.0.0.1:{port}/v1/logs"
        )),
        "{out}"
    );
    assert!(out.contains("Dropped 1 batch(es) so far:"), "{out}");
    for screen in [&out, &err] {
        assert!(
            !screen.contains("SECRET") && !screen.contains("session.id"),
            "{screen}"
        );
    }
    // No payload on disk, anywhere in the data folder.
    for file in files_in(rig.dir.path()) {
        let bytes = fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("SECRET-PROMPT"),
            "{}",
            file.display()
        );
    }
}

#[test]
fn the_program_refuses_a_busy_port_an_unpaired_device_and_a_missing_otel() {
    let rig = Rig::new();
    let run = |port: Option<u16>| {
        let mut args = vec!["relay".to_owned()];
        if let Some(p) = port {
            args.extend(["--port".into(), p.to_string()]);
        }
        let out = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
            .args(&args)
            .env("SPENDBRANCH_HOME", rig.dir.path())
            .env("SPENDBRANCH_KEY_STORE", "file")
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    let (code, said) = run(Some(4999));
    assert_eq!(code, Some(1));
    assert!(said.contains("not connected"), "{said}");

    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let (code, said) = run(None);
    assert_eq!(code, Some(1));
    assert!(
        said.contains("Run `spendbranch otel claude-code` first"),
        "{said}"
    );

    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let (_, _) = agent_endpoint(&rig);
    let (code, said) = run(Some(port));
    assert_eq!(code, Some(1));
    assert!(
        said.contains(&format!("Cannot listen on 127.0.0.1:{port}")) && said.contains("--port"),
        "{said}"
    );
    // The agent's own port taken by someone else: said plainly, with the way out.
    rig.store()
        .meta_set("relay_port", &port.to_string())
        .unwrap();
    let (code, said) = run(None);
    assert_eq!(code, Some(1));
    assert!(
        said.contains("Another program holds the port Claude Code sends to")
            && said.contains("run `spendbranch otel claude-code` again"),
        "{said}"
    );
}
