//! What may be said to the server, and how its answers are read.
use spendbranch::api::{
    ApiFailure, PairingStarted, PollOutcome, Polled, ServerUrl, ServerUrlError, is_plain_code,
};

fn parse(text: &str) -> Result<ServerUrl, ServerUrlError> {
    ServerUrl::parse(text)
}

#[test]
fn remote_servers_need_https() {
    for ok in [
        "https://api.example.com",
        "https://api.example.com/",
        "https://api.example.com:8443",
        "HTTPS://Api.Example.com",
        "https://[2001:db8::1]:8443",
    ] {
        assert!(parse(ok).is_ok(), "{ok}");
    }
    for bad in [
        "http://api.example.com",
        "http://example.com:3000",
        "http://127.0.0.2:3000",
        "http://10.0.0.1",
        "http://[::2]",
        "http://localhost.example.com",
        "http://127.0.0.1.evil.com",
    ] {
        assert_eq!(parse(bad), Err(ServerUrlError::InsecureRemote), "{bad}");
    }
}

#[test]
fn plain_http_is_for_this_machine_only() {
    for ok in [
        "http://localhost:3000",
        "http://127.0.0.1:3000",
        "http://[::1]:3000",
        "http://LOCALHOST",
        "http://localhost/",
    ] {
        assert!(parse(ok).is_ok(), "{ok}");
        assert!(parse(ok).unwrap().is_loopback());
    }
}

#[test]
fn tricks_that_fool_a_host_check_are_malformed() {
    for bad in [
        "",
        "localhost:3000",
        "ftp://localhost",
        "file:///etc/passwd",
        "https://",
        "https:///x",
        "http://localhost@evil.com",
        "http://evil.com#@localhost",
        "http://evil.com\\@localhost",
        "http://localhost:3000/api",
        "https://h/path",
        "https://h//",
        "https://h?x=1",
        "https://h#f",
        "https://h:0",
        "https://h:65536",
        "https://h:80a",
        "https://h:",
        "https://a b.com",
        "https://exämple.com",
        "https://-a.com",
        "https://a..com",
        "https://a.com.",
        "https://[::1",
        "https://[]",
        "https://[xyz]",
        "https://u:p@h.com",
        " https://h.com",
        "https://h.com\n",
    ] {
        assert_eq!(parse(bad), Err(ServerUrlError::Malformed), "{bad:?}");
    }
}

#[test]
fn the_base_has_no_trailing_slash_or_default_port() {
    assert_eq!(
        parse("https://API.example.com/").unwrap().base(),
        "https://api.example.com"
    );
    assert_eq!(
        parse("https://api.example.com:443").unwrap().base(),
        "https://api.example.com"
    );
    assert_eq!(
        parse("http://localhost:80").unwrap().base(),
        "http://localhost"
    );
    assert_eq!(
        parse("http://localhost:3000").unwrap().base(),
        "http://localhost:3000"
    );
}

#[test]
fn links_must_lead_to_the_server_origin() {
    let server = parse("https://app.example.com").unwrap();
    for same in [
        "https://app.example.com/devices/pair",
        "https://APP.example.com:443/devices/pair?code=BCDF-GHJK",
        "https://app.example.com",
    ] {
        assert!(server.same_origin(same), "{same}");
    }
    for other in [
        "http://app.example.com/devices/pair",
        "https://app.example.com:8443/",
        "https://evil.com/",
        "https://app.example.com.evil.com/",
        "https://app.example.com@evil.com/",
        "javascript:alert(1)",
        "//app.example.com",
        "",
    ] {
        assert!(!server.same_origin(other), "{other}");
    }
}

fn started(edit: impl FnOnce(&mut PairingStarted)) -> Option<PairingStarted> {
    let mut p: PairingStarted = serde_json::from_str(
        r#"{"deviceCode":"abcDEF_-123","userCode":"BCDF-GHJK","verificationUri":"https://app.example.com/devices/pair",
            "verificationUriComplete":"https://app.example.com/devices/pair?code=BCDF-GHJK","expiresIn":600,"interval":5}"#,
    )
    .unwrap();
    edit(&mut p);
    p.checked(&parse("https://app.example.com").unwrap())
}

#[test]
fn a_good_pairing_answer_is_accepted() {
    assert!(started(|_| {}).is_some());
}

#[test]
fn a_bad_pairing_answer_is_refused() {
    assert!(
        started(|p| p.user_code = "BCDF-GHJA".into()).is_none(),
        "A is not in the alphabet"
    );
    assert!(started(|p| p.user_code = "BCDFGHJK".into()).is_none());
    assert!(started(|p| p.user_code = "\x1b[2JCDF-GH".into()).is_none());
    assert!(started(|p| p.device_code = "a b".into()).is_none());
    assert!(started(|p| p.device_code = String::new()).is_none());
    assert!(started(|p| p.expires_in = 0).is_none());
    assert!(started(|p| p.expires_in = 86_400).is_none());
    assert!(started(|p| p.interval = 0).is_none());
    assert!(started(|p| p.verification_uri = "https://evil.com/devices/pair".into()).is_none());
    assert!(started(|p| p.verification_uri_complete = "https://evil.com/?c=1".into()).is_none());
    assert!(started(|p| p.verification_uri_complete.push_str("\n--flag")).is_none());
}

#[test]
fn verification_links_cannot_carry_terminal_control_text() {
    // The link is printed: an escape sequence in its path could rewrite the screen.
    for tail in [
        "\x1b[2J",
        "\x1b]8;;http://evil\x07",
        "\u{202e}gpj",
        "\r",
        "\u{85}",
    ] {
        assert!(
            started(|p| p.verification_uri.push_str(tail)).is_none(),
            "uri {tail:?}"
        );
        assert!(
            started(|p| p.verification_uri_complete.push_str(tail)).is_none(),
            "complete {tail:?}"
        );
    }
    assert!(
        started(|p| p.verification_uri_complete =
            format!("https://app.example.com/{}", "a".repeat(3000)))
        .is_none()
    );
}

fn poll(json: &str) -> Option<Polled> {
    serde_json::from_str::<PollOutcome>(json).ok()?.checked()
}

#[test]
fn poll_answers_are_read_by_status() {
    assert_eq!(
        poll(r#"{"status":"authorization_pending"}"#),
        Some(Polled::Pending)
    );
    assert_eq!(poll(r#"{"status":"slow_down"}"#), Some(Polled::SlowDown));
    assert_eq!(poll(r#"{"status":"expired_token"}"#), Some(Polled::Expired));
    assert_eq!(poll(r#"{"status":"access_denied"}"#), Some(Polled::Denied));
    assert_eq!(poll(r#"{"status":"unheard_of"}"#), None);
    assert_eq!(poll(r#"{}"#), None);
    let Some(Polled::Approved(a)) = poll(
        r#"{"status":"approved","deviceId":"dev-1","expiresAt":"2027-02-01T00:00:00Z","organizationId":"org_1","organizationName":"Acme Inc."}"#,
    ) else {
        panic!("approved expected")
    };
    assert_eq!(a.device_id.as_str(), "dev-1");
    assert_eq!(a.organization_name, "Acme Inc.");
}

#[test]
fn an_approval_with_bad_data_is_refused() {
    let approved = |device: &str, at: &str, org: &str, name: &str| {
        poll(&format!(
            r#"{{"status":"approved","deviceId":"{device}","expiresAt":"{at}","organizationId":"{org}","organizationName":"{name}"}}"#
        ))
    };
    assert!(approved("dev-1", "2027-02-01T00:00:00Z", "org", "Acme").is_some());
    assert!(approved("a b", "2027-02-01T00:00:00Z", "org", "Acme").is_none());
    assert!(approved("dev-1", "next tuesday", "org", "Acme").is_none());
    assert!(approved("dev-1", "2027-02-01T00:00:00Z", "", "Acme").is_none());
    assert!(approved("dev-1", "2027-02-01T00:00:00Z", "org", "").is_none());
    // what the terminal would obey, or what flips the text direction
    assert!(approved("dev-1", "2027-02-01T00:00:00Z", "org", "Acme\\u001b[31m").is_none());
    assert!(approved("dev-1", "2027-02-01T00:00:00Z", "org", "Acme\\u202eevil").is_none());
    assert!(approved("dev-1", "2027-02-01T00:00:00Z", "org", &"x".repeat(201)).is_none());
    // and a token is not part of the contract any more: nothing for it to be read into
    assert!(poll(r#"{"status":"approved","token":"sbd_x"}"#).is_none());
}

#[test]
fn failures_are_built_from_what_the_server_said() {
    let f = ApiFailure::from_response(
        429,
        br#"{"error":{"code":"RATE_LIMITED","message":"slow"}}"#,
        Some("60"),
        None,
    );
    assert_eq!(
        (
            f.status,
            f.code.as_str(),
            f.retry_after_secs,
            f.server_time_ms
        ),
        (429, "RATE_LIMITED", Some(60), None)
    );

    let f = ApiFailure::from_response(
        401,
        br#"{"error":{"code":"CLOCK_SKEW"}}"#,
        None,
        Some("1760000000000"),
    );
    assert_eq!(
        (f.code.as_str(), f.server_time_ms),
        ("CLOCK_SKEW", Some(1_760_000_000_000))
    );

    // not the documented shape, or a code that is not plain: INTERNAL, and the text is not kept
    for body in [
        &b""[..],
        b"<html>",
        br#"{"error":"x"}"#,
        br#"{"error":{"code":"\u001b[2J"}}"#,
        br#"{"error":{"code":"lower"}}"#,
    ] {
        assert_eq!(
            ApiFailure::from_response(500, body, None, None).code,
            "INTERNAL"
        );
    }
}

#[test]
fn retry_after_and_server_time_are_bounded() {
    let f = |ra: Option<&str>, st: Option<&str>| ApiFailure::from_response(429, b"", ra, st);
    assert_eq!(f(Some("0"), None).retry_after_secs, None);
    assert_eq!(f(Some("-5"), None).retry_after_secs, None);
    assert_eq!(f(Some("soon"), None).retry_after_secs, None);
    assert_eq!(
        f(Some("Wed, 21 Oct 2026 07:28:00 GMT"), None).retry_after_secs,
        None
    );
    assert_eq!(f(Some("99999999"), None).retry_after_secs, Some(86_400));
    assert_eq!(f(None, Some("abc")).server_time_ms, None);
    assert_eq!(f(None, Some("-1")).server_time_ms, None);
}

#[test]
fn plain_codes() {
    assert!(is_plain_code("CLOCK_SKEW"));
    assert!(!is_plain_code(""));
    assert!(!is_plain_code("a"));
    assert!(!is_plain_code(&"A".repeat(65)));
}

#[test]
fn failures_do_not_print_server_text() {
    let f = ApiFailure::from_response(
        400,
        br#"{"error":{"code":"VALIDATION_FAILED","message":"secret detail"}}"#,
        None,
        None,
    );
    assert!(!f.to_string().contains("secret detail"));
}
