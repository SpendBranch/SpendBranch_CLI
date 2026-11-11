//! The client against a real local server that checks signatures like the API does.
#[path = "common/fake_server.rs"]
mod fake_server;

use fake_server::{FakeServer, WINDOW_MS};
use serde_json::{Value, json};
use spendbranch::api::{
    ApiFailure, PairingStarted, PollOutcome, PollToken, Polled, ServerUrl, StartPairing,
};
use spendbranch::auth::{DeviceId, DeviceKey, sign_request};
use spendbranch::infra::{
    clock::{Clock, SystemClock},
    http::{Client, Signer},
    keys,
};
use std::{
    net::TcpListener,
    sync::atomic::{AtomicI64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const VERSION: &str = "1.2.3";

fn real_now() -> i64 {
    SystemClock.now_ms()
}

/// The real time moved by a fixed amount: a computer whose clock is wrong.
struct Skewed(i64);
impl Clock for Skewed {
    fn now_ms(&self) -> i64 {
        real_now() + self.0
    }
}

/// A clock that never moves, so two attempts get the same timestamp.
struct Frozen(AtomicI64);
impl Clock for Frozen {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn client(server: &FakeServer, clock: impl Clock + 'static) -> Client {
    Client::with_clock(
        ServerUrl::parse(&server.base).unwrap(),
        VERSION,
        Box::new(clock),
    )
}

/// A server with one registered device, and the key and id to sign as it.
fn paired() -> (FakeServer, DeviceKey, DeviceId) {
    let server = FakeServer::start();
    let key = keys::generate().unwrap();
    server.register("dev-1", &key.public_key());
    (server, key, DeviceId::new("dev-1").unwrap())
}

fn status(client: &Client, key: &DeviceKey, id: &DeviceId) -> Result<Value, ApiFailure> {
    client.get("/api/cli/v1/status", Some(&Signer { device: id, key }))
}

/// A request signed by hand, for the cases the client would never produce.
struct Raw {
    method: &'static str,
    path: &'static str,
    timestamp: i64,
    nonce: [u8; 16],
    sign_body: Vec<u8>,
    send_body: Vec<u8>,
    claim_device: &'static str,
    signer: &'static str,
}

impl Raw {
    fn status(timestamp: i64, nonce: u8) -> Self {
        Self {
            method: "GET",
            path: "/api/cli/v1/status",
            timestamp,
            nonce: [nonce; 16],
            sign_body: vec![],
            send_body: vec![],
            claim_device: "dev-1",
            signer: "dev-1",
        }
    }

    fn send(&self, server: &FakeServer, keys: &[(&str, &DeviceKey)]) -> (u16, String, Option<i64>) {
        let key = keys.iter().find(|(n, _)| *n == self.signer).unwrap().1;
        let mut h = sign_request(
            key,
            &DeviceId::new(self.signer).unwrap(),
            self.method,
            self.path,
            self.timestamp,
            &self.nonce,
            &self.sign_body,
        );
        h.device = self.claim_device.into();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut req = ureq::http::Request::builder()
            .method(self.method)
            .uri(format!("{}{}", server.base, self.path));
        for (k, v) in h.pairs() {
            req = req.header(k, v);
        }
        let mut res = agent
            .run(req.body(self.send_body.clone()).unwrap())
            .unwrap();
        let time = res
            .headers()
            .get("x-sb-server-time")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let status = res.status().as_u16();
        let body: Value =
            serde_json::from_str(&res.body_mut().read_to_string().unwrap()).unwrap_or(Value::Null);
        (
            status,
            body["error"]["code"].as_str().unwrap_or("").to_owned(),
            time,
        )
    }
}

// --- pairing and a full life of a device -----------------------------------------------

#[test]
fn a_device_pairs_signs_and_logs_out() {
    let server = FakeServer::start();
    let c = client(&server, SystemClock);
    let key = keys::generate().unwrap();

    let started: PairingStarted = c
        .post(
            "/api/cli/v1/device-codes",
            &StartPairing {
                installation_id: "inst-1",
                cli_version: Some(VERSION),
                public_key: &key.public_key(),
            },
            None,
        )
        .unwrap();
    assert!(
        started
            .checked(&ServerUrl::parse("http://127.0.0.1").unwrap())
            .is_some()
    );

    let poll = |c: &Client| -> Polled {
        c.post::<_, PollOutcome>(
            "/api/cli/v1/tokens",
            &PollToken {
                device_code: "dc-abcdefghijklmnopqrstuvwxyz",
            },
            None,
        )
        .unwrap()
        .checked()
        .unwrap()
    };
    assert_eq!(poll(&c), Polled::Pending);
    let device_id = server.approve();
    let Polled::Approved(approval) = poll(&c) else {
        panic!("approved expected")
    };
    assert_eq!(approval.device_id.as_str(), device_id);

    let id = approval.device_id;
    assert_eq!(status(&c, &key, &id).unwrap()["organizationName"], "Acme");
    c.post_empty(
        "/api/cli/v1/logout",
        Some(&Signer {
            device: &id,
            key: &key,
        }),
    )
    .unwrap();
    assert_eq!(server.revoked_reason(&device_id).as_deref(), Some("logout"));
    assert_eq!(status(&c, &key, &id).unwrap_err().code, "UNAUTHENTICATED");
}

#[test]
fn the_pairing_request_carries_the_public_key_and_nothing_secret() {
    let server = FakeServer::start();
    let key = keys::generate().unwrap();
    let c = client(&server, SystemClock);
    let _: Value = c
        .post(
            "/api/cli/v1/device-codes",
            &StartPairing {
                installation_id: "inst-1",
                cli_version: None,
                public_key: &key.public_key(),
            },
            None,
        )
        .unwrap();
    let seen = server.seen();
    let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(
        body,
        json!({"installationId": "inst-1", "cliVersion": null, "publicKey": key.public_key()})
    );
    assert_eq!(
        seen[0].headers["user-agent"],
        format!("spendbranch-cli/{VERSION}")
    );
    assert_eq!(seen[0].headers["content-type"], "application/json");
    assert!(
        !seen[0].headers.contains_key("authorization"),
        "there is no token"
    );
    assert!(
        !seen[0].headers.keys().any(|k| k.starts_with("x-sb-")),
        "pairing is not signed"
    );
}

#[test]
fn no_request_ever_carries_the_private_key() {
    let (server, key, id) = paired();
    let c = client(&server, SystemClock);
    status(&c, &key, &id).unwrap();
    let _: Value = c
        .post(
            "/api/ingest/v1/events",
            &json!({"observations": []}),
            Some(&Signer {
                device: &id,
                key: &key,
            }),
        )
        .unwrap();
    let secret = key.to_text();
    for r in server.seen() {
        let all = format!("{:?}{}", r.headers, String::from_utf8_lossy(&r.body));
        assert!(!all.contains(secret.as_str()));
    }
}

// --- what the server must refuse, and with which code ----------------------------------

#[test]
fn a_changed_body_is_refused() {
    let (server, key, _) = paired();
    let mut raw = Raw::status(real_now(), 1);
    (raw.method, raw.path) = ("POST", "/api/ingest/v1/events");
    raw.sign_body = br#"{"observations":[]}"#.to_vec();
    raw.send_body = br#"{"observations":[{"tokens":999999}]}"#.to_vec();
    let (status, code, _) = raw.send(&server, &[("dev-1", &key)]);
    assert_eq!((status, code.as_str()), (401, "UNAUTHENTICATED"));
    assert!(
        server.device_active("dev-1"),
        "a bad signature must not revoke: anyone could send one"
    );

    raw.send_body = raw.sign_body.clone();
    assert_eq!(
        raw.send(&server, &[("dev-1", &key)]).0,
        200,
        "the untouched body goes through"
    );
}

#[test]
fn another_device_cannot_speak_for_this_one() {
    let (server, key, _) = paired();
    let other = keys::generate().unwrap();
    server.register("dev-2", &other.public_key());
    let keys = [("dev-1", &key), ("dev-2", &other)];

    // dev-2's valid key, but claiming to be dev-1
    let mut raw = Raw::status(real_now(), 2);
    (raw.signer, raw.claim_device) = ("dev-2", "dev-1");
    assert_eq!(raw.send(&server, &keys).1, "UNAUTHENTICATED");
    assert!(server.device_active("dev-1") && server.device_active("dev-2"));

    // a device the server never heard of
    let mut raw = Raw::status(real_now(), 3);
    raw.claim_device = "dev-9";
    assert_eq!(raw.send(&server, &keys).1, "UNAUTHENTICATED");

    // and each one on its own is fine
    let mut raw = Raw::status(real_now(), 4);
    (raw.signer, raw.claim_device) = ("dev-2", "dev-2");
    assert_eq!(raw.send(&server, &keys).0, 200);
}

#[test]
fn a_repeated_nonce_is_refused_and_revokes_the_device() {
    let (server, key, _) = paired();
    let raw = Raw::status(real_now(), 5);
    assert_eq!(raw.send(&server, &[("dev-1", &key)]).0, 200);
    let (status, code, _) = raw.send(&server, &[("dev-1", &key)]);
    assert_eq!((status, code.as_str()), (401, "UNAUTHENTICATED"));
    assert_eq!(
        server.revoked_reason("dev-1").as_deref(),
        Some("auto:replay")
    );
    // revoked: even a fresh, valid request is refused now
    assert_eq!(
        Raw::status(real_now(), 6)
            .send(&server, &[("dev-1", &key)])
            .1,
        "UNAUTHENTICATED"
    );
}

#[test]
fn a_timestamp_outside_the_window_is_refused_with_the_server_time() {
    let (server, key, _) = paired();
    for (n, off) in [
        (10u8, WINDOW_MS + 60_000),
        (11, -(WINDOW_MS + 60_000)),
        (12, 24 * 3_600_000),
    ] {
        let (status, code, time) =
            Raw::status(real_now() + off, n).send(&server, &[("dev-1", &key)]);
        assert_eq!((status, code.as_str()), (401, "CLOCK_SKEW"), "{off}");
        assert!(
            (time.unwrap() - real_now()).abs() < 5_000,
            "the header tells the server's time"
        );
    }
    assert!(
        server.device_active("dev-1"),
        "a wrong clock must not revoke"
    );
    // just inside the window is accepted
    assert_eq!(
        Raw::status(real_now() + WINDOW_MS - 30_000, 13)
            .send(&server, &[("dev-1", &key)])
            .0,
        200
    );
}

#[test]
fn the_checks_run_in_the_order_of_the_spec() {
    let (server, key, _) = paired();
    let keys = [("dev-1", &key)];
    // bad signature AND bad time: the signature comes first, so no hint about the clock
    let mut raw = Raw::status(real_now() + 3_600_000, 20);
    raw.send_body = b"x".to_vec();
    let (_, code, time) = raw.send(&server, &keys);
    assert_eq!((code.as_str(), time), ("UNAUTHENTICATED", None));
    // a valid request replayed with a stale time is a clock problem, not a replay: no revocation
    let stale = Raw::status(real_now() - 3_600_000, 21);
    assert_eq!(stale.send(&server, &keys).1, "CLOCK_SKEW");
    assert_eq!(stale.send(&server, &keys).1, "CLOCK_SKEW");
    assert!(server.device_active("dev-1"));
    // a bad signature does not use up the nonce
    let mut bad = Raw::status(real_now(), 22);
    bad.send_body = b"x".to_vec();
    assert_eq!(bad.send(&server, &keys).1, "UNAUTHENTICATED");
    assert_eq!(Raw::status(real_now(), 22).send(&server, &keys).0, 200);
}

// --- what the client does ---------------------------------------------------------------

#[test]
fn a_skewed_clock_is_reported_with_the_servers_time() {
    let (server, key, id) = paired();
    let c = client(&server, Skewed(3_600_000));
    let err = status(&c, &key, &id).unwrap_err();
    assert_eq!((err.status, err.code.as_str()), (401, "CLOCK_SKEW"));
    assert!((err.server_time_ms.unwrap() - real_now()).abs() < 5_000);
    assert!(server.device_active("dev-1"));
}

#[test]
fn every_attempt_has_a_new_nonce_even_with_the_same_timestamp() {
    let (server, key, id) = paired();
    let c = client(&server, Frozen(AtomicI64::new(real_now())));
    for _ in 0..3 {
        status(&c, &key, &id).unwrap(); // a retry is just the same call again
    }
    let seen = server.seen();
    let nonces: std::collections::HashSet<_> = seen
        .iter()
        .map(|r| r.headers["x-sb-nonce"].clone())
        .collect();
    assert_eq!(
        nonces.len(),
        3,
        "a reused nonce would make the server revoke us"
    );
    assert!(server.device_active("dev-1"));
}

#[test]
fn every_attempt_has_a_new_timestamp_from_the_clock() {
    let (server, key, id) = paired();
    let clock = std::sync::Arc::new(AtomicI64::new(real_now()));
    struct Shared(std::sync::Arc<AtomicI64>);
    impl Clock for Shared {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    let c = client(&server, Shared(clock.clone()));
    status(&c, &key, &id).unwrap();
    let first = real_now();
    clock.store(first + 1_000, Ordering::SeqCst);
    status(&c, &key, &id).unwrap();
    let times: Vec<i64> = server
        .seen()
        .iter()
        .map(|r| r.headers["x-sb-timestamp"].parse().unwrap())
        .collect();
    assert_eq!(times[1] - times[0], 1_000 + (first - times[0]));
    assert_ne!(times[0], times[1]);
}

#[test]
fn a_redirect_is_never_followed() {
    let elsewhere = TcpListener::bind("127.0.0.1:0").unwrap();
    elsewhere.set_nonblocking(true).unwrap();
    let server = FakeServer::start();
    server.state.lock().unwrap().redirect_to =
        format!("http://{}/steal", elsewhere.local_addr().unwrap());
    let c = client(&server, SystemClock);
    let err = c.get::<Value>("/redirect", None).unwrap_err();
    assert_eq!((err.status, err.code.as_str()), (302, "INTERNAL"));
    assert!(
        elsewhere.accept().is_err(),
        "nothing connected to the redirect target"
    );
}

#[test]
fn http_to_a_remote_host_is_refused_before_any_connection() {
    // 127.0.0.2 is this machine but not on the allowed list, so it counts as remote.
    // Where it cannot be bound (macOS), the address check alone is what is tested.
    let listener = TcpListener::bind("127.0.0.2:0").ok();
    let port = listener
        .as_ref()
        .map_or(3000, |l| l.local_addr().unwrap().port());
    for url in [
        format!("http://127.0.0.2:{port}"),
        format!("http://example.com:{port}"),
    ] {
        assert!(ServerUrl::parse(&url).is_err(), "{url}");
    }
    if let Some(l) = listener {
        l.set_nonblocking(true).unwrap();
        assert!(l.accept().is_err(), "nothing connected");
    }
}

#[test]
fn failures_on_the_wire_become_api_failures() {
    let server = FakeServer::start();
    let c = client(&server, SystemClock);
    let get = |p: &str| c.get::<Value>(p, None).unwrap_err();

    let f = get("/limited");
    assert_eq!(
        (f.status, f.code.as_str(), f.retry_after_secs),
        (429, "RATE_LIMITED", Some(60))
    );
    assert!(!f.to_string().contains("must not show"));
    let f = get("/broken");
    assert_eq!((f.status, f.code.as_str()), (500, "INTERNAL"));
    let f = get("/garbage");
    assert_eq!((f.status, f.code.as_str()), (200, "BAD_RESPONSE"));
    let f = get("/nope");
    assert_eq!((f.status, f.code.as_str()), (404, "NOT_FOUND"));
    // an answer larger than any real one is not read
    assert_eq!(get("/huge").code, "NETWORK");
}

#[test]
fn an_unreachable_server_is_a_network_failure() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }; // closed again
    let c = Client::new(
        ServerUrl::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
        VERSION,
    );
    let f = c.get::<Value>("/api/cli/v1/status", None).unwrap_err();
    assert_eq!((f.status, f.code.as_str()), (0, "NETWORK"));
}

#[test]
fn the_clock_is_the_real_one_by_default() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!((SystemClock.now_ms() - now).abs() < 5_000);
}
