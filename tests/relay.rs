//! The OTLP relay: the lenient filter against the shared fixtures, every row of the answer
//! table over a real socket, and the real program forwarding, signed, to the fake server.
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use rig::{Rig, authorize};
use serde_json::{Value, json};
use spendbranch::infra::relay_server::{self, Event};
use spendbranch::relay::{self, Head, RateLimit};
use spendbranch::sources::otlp;
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

fn json_of(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn fixtures() -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/otlp");
    let mut dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    dirs.sort();
    dirs
}

// --- the filter -----------------------------------------------------------------------

#[test]
fn the_lenient_payload_is_exactly_what_the_shared_corpus_expects() {
    let cases = fixtures();
    assert!(cases.len() >= 10, "the corpus went missing");
    for dir in cases {
        let input = json_of(&dir.join("input.json"));
        let expected = json_of(&dir.join("expected.json"));
        let got = otlp::lenient(&input);
        assert_eq!(got.payload, expected["relayPayload"], "{}", dir.display());
    }
}

fn record(name: &str, attributes: Value) -> Value {
    json!({"eventName": name, "timeUnixNano": "1767261601000000000", "attributes": attributes})
}

fn payload(records: Vec<Value>) -> Value {
    json!({"resourceLogs": [{"resource": {"attributes": [{"key": "user.email", "value": {"stringValue": "a@b.c"}}]},
                             "scopeLogs": [{"logRecords": records}]}]})
}

#[test]
fn nothing_but_the_declared_attributes_of_an_api_request_gets_through() {
    let body = payload(vec![
        record(
            "api_request",
            json!([
                {"key": "model", "value": {"stringValue": "m"}},
                {"key": "prompt", "value": {"stringValue": "SECRET-PROMPT"}},
                {"key": "user.email", "value": {"stringValue": "SECRET-EMAIL"}},
                {"key": "input_tokens", "value": {"intValue": 5}},
                {"key": "output_tokens", "value": {"doubleValue": 1.5}},
                {"key": "cache_read_tokens", "value": {"boolValue": true}},
                {"key": "session.id", "value": {"stringValue": "s", "intValue": "1"}},
            ]),
        ),
        record(
            "user_prompt",
            json!([{"key": "model", "value": {"stringValue": "SECRET-OTHER"}}]),
        ),
        json!("not a record"),
    ]);
    let f = otlp::lenient(&body);
    assert_eq!((f.kept, f.dropped, f.ignored), (1, 2, 0));
    let text = f.payload.to_string();
    for secret in ["SECRET", "user.email", "a@b.c", "resource\":{"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let attrs = f
        .payload
        .pointer("/resourceLogs/0/scopeLogs/0/logRecords/0/attributes")
        .unwrap();
    let keys: Vec<&str> = attrs
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["model", "input_tokens", "session.id"]);
    assert_eq!(
        attrs[2]["value"],
        json!({"stringValue": "s"}),
        "one kind of value per attribute"
    );
}

#[test]
fn the_name_comes_from_the_field_then_the_attribute_then_the_body() {
    let by_body = json!({"body": {"stringValue": "claude_code.api_request"}, "attributes": []});
    let by_attr =
        json!({"attributes": [{"key": "event.name", "value": {"stringValue": "api_request"}}]});
    let other = json!({"eventName": "tool_result", "attributes": [{"key": "event.name", "value": {"stringValue": "api_request"}}]});
    let f = otlp::lenient(&payload(vec![by_body, by_attr, other]));
    assert_eq!(
        (f.kept, f.dropped),
        (2, 1),
        "the field wins over the attribute"
    );
}

#[test]
fn text_over_256_characters_is_dropped_and_256_stays() {
    let long = "é".repeat(257);
    let fine = "é".repeat(256);
    let f = otlp::lenient(&payload(vec![record(
        "api_request",
        json!([{"key": "model", "value": {"stringValue": long}}, {"key": "query_source", "value": {"stringValue": fine}}]),
    )]));
    let attrs = f
        .payload
        .pointer("/resourceLogs/0/scopeLogs/0/logRecords/0/attributes")
        .unwrap();
    assert_eq!(attrs.as_array().unwrap().len(), 1);
    assert_eq!(attrs[0]["key"], "query_source");
}

#[test]
fn a_repeated_key_keeps_the_first_and_odd_numbers_and_times_are_dropped() {
    let f = otlp::lenient(&payload(vec![json!({
    "eventName": "api_request", "timeUnixNano": "12ab",
    "attributes": [
        {"key": "model", "value": {"stringValue": "first"}},
        {"key": "model", "value": {"stringValue": "second"}},
        {"key": "input_tokens", "value": {"intValue": "1e9"}},
        {"key": "output_tokens", "value": {"intValue": "-7"}},
        {"key": "cache_read_tokens", "value": {"intValue": "99999999999999999999"}},
        {"key": "request_id"},
        {"value": {"stringValue": "no key"}},
    ]})]));
    let r = f
        .payload
        .pointer("/resourceLogs/0/scopeLogs/0/logRecords/0")
        .unwrap();
    assert!(r.get("timeUnixNano").is_none());
    let attrs = r["attributes"].as_array().unwrap();
    assert_eq!(attrs.len(), 2);
    assert_eq!(attrs[0]["value"]["stringValue"], "first");
    assert_eq!(attrs[1]["value"]["intValue"], "-7");
}

#[test]
fn only_the_first_2000_records_are_read() {
    let many: Vec<Value> = (0..2005)
        .map(|_| record("api_request", json!([])))
        .collect();
    let f = otlp::lenient(&payload(many));
    assert_eq!((f.kept, f.ignored, f.dropped), (2000, 5, 0));
}

#[test]
fn whatever_the_shape_the_answer_is_a_well_formed_empty_payload() {
    for body in [
        json!(null),
        json!([]),
        json!("x"),
        json!({"resourceLogs": 3}),
        json!({"resourceLogs": [null, {"scopeLogs": "x"}, {"scopeLogs": [{"logRecords": {}}]}]}),
    ] {
        let f = otlp::lenient(&body);
        assert_eq!(f.kept, 0);
        assert_eq!(
            f.payload,
            json!({"resourceLogs": [{"scopeLogs": [{"logRecords": []}]}]}),
            "{body}"
        );
    }
}

// --- the rules ------------------------------------------------------------------------

fn good<'a>() -> Head<'a> {
    Head {
        peer_is_loopback: true,
        method: "POST",
        path: "/v1/logs",
        host: Some("127.0.0.1:4318"),
        origin: None,
        content_type: Some("application/json"),
        content_encoding: None,
        transfer_encoding: None,
        content_length: Some(10),
    }
}

#[test]
fn each_row_of_the_table_has_its_status() {
    assert_eq!(relay::check(&good()), Ok(10));
    let cases: Vec<(&str, Head, u16)> = vec![
        (
            "remote",
            Head {
                peer_is_loopback: false,
                ..good()
            },
            403,
        ),
        (
            "rebinding host",
            Head {
                host: Some("evil.example"),
                ..good()
            },
            403,
        ),
        (
            "host with a suffix",
            Head {
                host: Some("127.0.0.1.evil.example"),
                ..good()
            },
            403,
        ),
        (
            "no host",
            Head {
                host: None,
                ..good()
            },
            403,
        ),
        (
            "any origin",
            Head {
                origin: Some("http://localhost"),
                ..good()
            },
            403,
        ),
        (
            "GET",
            Head {
                method: "GET",
                ..good()
            },
            405,
        ),
        (
            "metrics",
            Head {
                path: "/v1/metrics",
                ..good()
            },
            404,
        ),
        (
            "query string",
            Head {
                path: "/v1/logs?x=1",
                ..good()
            },
            404,
        ),
        (
            "text",
            Head {
                content_type: Some("text/plain"),
                ..good()
            },
            415,
        ),
        (
            "no type",
            Head {
                content_type: None,
                ..good()
            },
            415,
        ),
        (
            "gzip",
            Head {
                content_encoding: Some("gzip"),
                ..good()
            },
            415,
        ),
        (
            "identity",
            Head {
                content_encoding: Some("identity"),
                ..good()
            },
            415,
        ),
        (
            "chunked",
            Head {
                transfer_encoding: Some("chunked"),
                content_length: None,
                ..good()
            },
            411,
        ),
        (
            "both",
            Head {
                transfer_encoding: Some("chunked"),
                ..good()
            },
            411,
        ),
        (
            "no length",
            Head {
                content_length: None,
                ..good()
            },
            411,
        ),
        (
            "too big",
            Head {
                content_length: Some(1024 * 1024 + 1),
                ..good()
            },
            413,
        ),
    ];
    for (name, head, status) in cases {
        assert_eq!(relay::check(&head), Err(status), "{name}");
    }
    for host in [
        "127.0.0.1",
        "localhost",
        "LOCALHOST:4318",
        "[::1]:9",
        "[::1]",
    ] {
        assert!(
            relay::check(&Head {
                host: Some(host),
                ..good()
            })
            .is_ok(),
            "{host}"
        );
    }
    for ct in [
        "application/json",
        "application/json; charset=utf-8",
        "Application/JSON",
    ] {
        assert!(
            relay::check(&Head {
                content_type: Some(ct),
                ..good()
            })
            .is_ok(),
            "{ct}"
        );
    }
    assert_eq!(
        relay::check(&Head {
            content_length: Some(1024 * 1024),
            ..good()
        }),
        Ok(1024 * 1024)
    );
    for host in ["localhost:", "localhost:12a", "[::1", "[::1]x"] {
        assert_eq!(
            relay::check(&Head {
                host: Some(host),
                ..good()
            }),
            Err(403),
            "{host}"
        );
    }
}

#[test]
fn at_most_600_batches_a_minute_in_fixed_windows() {
    let mut limit = RateLimit::default();
    let t0 = 1_000_000;
    assert!((0..600).all(|i| limit.allow(t0 + i)));
    assert!(!limit.allow(t0 + 700));
    assert!(!limit.allow(t0 + 59_000));
    assert!(limit.allow(t0 + 60_000), "a new window");
    assert!(
        limit.allow(t0 - 5_000),
        "a clock that went back starts over"
    );
}

#[test]
fn the_answer_counts_what_was_refused() {
    assert_eq!(relay::answer(0), "{}");
    assert_eq!(
        relay::answer(3),
        r#"{"partialSuccess":{"rejectedLogRecords":3}}"#
    );
    assert_eq!(relay::refused_by_server(&json!({})), 0);
    assert_eq!(
        relay::refused_by_server(&json!({"partialSuccess": {"rejectedLogRecords": 4}})),
        4
    );
    assert_eq!(
        relay::refused_by_server(&json!({"partialSuccess": {"rejectedLogRecords": "2"}})),
        2
    );
    assert_eq!(
        relay::refused_by_server(&json!({"partialSuccess": {"rejectedLogRecords": -1}})),
        0
    );
}

// --- the server over a real socket ----------------------------------------------------

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
            relay_server::serve(&listener, &stop2, &forward, &tx).unwrap();
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

    fn post(&self, headers: &str, body: &str) -> (u16, String) {
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
    assert!(matches!(relay.events.try_recv(), Ok(Event::Forwarded)));
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
            "{l}\r\nHost: 127.0.0.1:{p}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
        )
    };
    let answers = [
        ("GET", relay.raw(line("GET /v1/logs HTTP/1.1").as_bytes()).0, 405),
        ("other path", relay.raw(line("POST /v1/metrics HTTP/1.1").as_bytes()).0, 404),
        ("not HTTP", relay.raw(b"hello\r\n\r\n").0, 400),
        ("bad version", relay.raw(line("POST /v1/logs SPDY/3").as_bytes()).0, 400),
        ("rebinding", relay.raw(b"POST /v1/logs HTTP/1.1\r\nHost: evil.example\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").0, 403),
        ("origin", relay.post("Origin: http://localhost\r\n", "{}").0, 403),
        ("gzip", relay.post("Content-Encoding: gzip\r\n", "{}").0, 415),
        ("bad json", relay.post("", "{nope").0, 400),
        ("empty", relay.post("", "").0, 400),
        ("chunked", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n").as_bytes()).0, 411),
        ("huge announced", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nContent-Type: application/json\r\nContent-Length: 99999999\r\n\r\n").as_bytes()).0, 413),
        ("two lengths", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 5\r\n\r\n{{}}").as_bytes()).0, 400),
        ("signed length", relay.raw(format!("POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nContent-Type: application/json\r\nContent-Length: +2\r\n\r\n{{}}").as_bytes()).0, 400),
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
        "POST /v1/logs HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}TRAILING",
        relay.port,
        body.len()
    );
    assert_eq!(relay.raw(req.as_bytes()).0, 200);
}

#[test]
fn the_601st_batch_in_a_minute_is_refused() {
    let relay = Relay::start(|_| Ok(0));
    let mut last = 0;
    for _ in 0..601 {
        last = relay.post("", "{}").0;
        if last == 429 {
            break;
        }
    }
    assert_eq!(last, 429);
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_program(rig: &Rig, port: u16) -> Child {
    let child: std::process::Child = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
        .args(["relay", "--port", &port.to_string()])
        .env("SPENDBRANCH_HOME", rig.dir.path())
        .env("SPENDBRANCH_KEY_STORE", "file")
        .env("HOME", rig.dir.path())
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

fn post_to(port: u16, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(stream, "POST /v1/logs HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
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
    let port = free_port();
    let child = start_program(&rig, port);

    let (status, body) = post_to(port, &sample());
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
    assert_eq!(post_to(port, &sample()).0, 502);

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
fn the_program_refuses_a_busy_port_and_an_unpaired_device() {
    let rig = Rig::new();
    let out = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
        .args(["relay", "--port", "4999"])
        .env("SPENDBRANCH_HOME", rig.dir.path())
        .env("SPENDBRANCH_KEY_STORE", "file")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("not connected"));

    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let out = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
        .args(["relay", "--port", &port.to_string()])
        .env("SPENDBRANCH_HOME", rig.dir.path())
        .env("SPENDBRANCH_KEY_STORE", "file")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(&format!("Cannot listen on 127.0.0.1:{port}")),
        "{said}"
    );
}

// --- `otel` ---------------------------------------------------------------------------

#[test]
fn otel_writes_a_private_environment_file_with_no_secret_in_it() {
    let rig = Rig::new();
    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let out = rig.run(&["otel", "claude-code"]);
    assert_eq!(out.code, 0, "{}", out.text);
    let file = rig.dir.path().join(if cfg!(windows) {
        "otel-claude_code.ps1"
    } else {
        "otel-claude_code.env"
    });
    let text = fs::read_to_string(&file).unwrap();
    for line in [
        "CLAUDE_CODE_ENABLE_TELEMETRY",
        "OTEL_LOGS_EXPORTER",
        "OTEL_METRICS_EXPORTER",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
    assert!(
        text.contains("OTEL_METRICS_EXPORTER=none")
            || text.contains("OTEL_METRICS_EXPORTER = 'none'"),
        "{text}"
    );
    assert!(text.contains("http://127.0.0.1:4318/v1/logs"));
    assert!(
        !text.contains("OTEL_EXPORTER_OTLP_HEADERS"),
        "no header, no credential: {text}"
    );
    // The device key is nowhere in it.
    let key = fs::read_to_string(rig.dir.path().join("keys").join("active.key")).or_else(|_| {
        fs::read_dir(rig.dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().contains("key"))
            .map(fs::read_to_string)
            .unwrap()
    });
    if let Ok(key) = key {
        let key = key.trim().to_owned();
        assert!(key.len() < 8 || !text.contains(&key));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        out.text.contains(&format!("source '{}'", file.display())) || cfg!(windows),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("keep `spendbranch relay` running"),
        "{}",
        out.text
    );
    assert!(out.text.contains("OTEL_LOG_USER_PROMPTS"), "{}", out.text);
    // Again, over a longer old file: rewritten whole, with no tail of the old one.
    fs::write(&file, "x".repeat(5000)).unwrap();
    assert_eq!(rig.run(&["otel", "claude_code"]).code, 0);
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn otel_refuses_codex_and_an_unpaired_device() {
    let rig = Rig::new();
    let out = rig.run(&["otel", "codex"]);
    assert_eq!(out.code, 2);
    assert!(
        out.text
            .contains("Codex is not collected through OpenTelemetry"),
        "{}",
        out.text
    );
    let out = rig.run(&["otel", "claude-code"]);
    assert_eq!(out.code, 1);
    assert!(!rig.dir.path().join("otel-claude_code.env").exists());
}

#[test]
fn the_command_announces_the_address_and_ends_when_told_to_stop() {
    let rig = Rig::new();
    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let port = free_port();
    let out = rig.run(&["relay", "--port", &port.to_string()]); // the fake system says: stop now
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains(&format!(
            "Relay listening on http://127.0.0.1:{port}/v1/logs and forwarding to {}",
            rig.server.base
        )),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("point the agent's OTEL_EXPORTER_OTLP_LOGS_ENDPOINT at this port"),
        "{}",
        out.text
    );
}
