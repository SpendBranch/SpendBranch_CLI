//! The relay's table of answers, one row at a time, as pure rules.
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use serde_json::json;
use spendbranch::relay::{self, Head, RateLimit};

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
