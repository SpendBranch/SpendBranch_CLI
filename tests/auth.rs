//! The signature of every request, and the key that makes it.
use spendbranch::auth::{DeviceId, DeviceKey, sign_request, signing_string};

fn key() -> DeviceKey {
    DeviceKey::from_seed(&std::array::from_fn(|i| i as u8))
}

fn device() -> DeviceId {
    DeviceId::new("dev_1").unwrap()
}

const NONCE: [u8; 16] = [
    100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
];
const BODY: &[u8] = br#"{"deviceCode":"abc"}"#;
const PATH: &str = "/api/cli/v1/tokens";

/// Computed by Python's `cryptography` (OpenSSL), not by this crate: if the signed text or
/// the encoding drifts from the spec, this is what notices.
#[test]
fn signature_matches_an_independent_implementation() {
    assert_eq!(
        key().public_key(),
        "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg"
    );
    let h = sign_request(
        &key(),
        &device(),
        "POST",
        PATH,
        1_760_000_000_000,
        &NONCE,
        BODY,
    );
    assert_eq!(h.nonce, "ZGVmZ2hpamtsbW5vcHFycw");
    assert_eq!(
        h.signature,
        "bh_c-c-Rk41kEOSpfwlJpreiBxj0zsfi0xkgMsvPNSU_lPKubqB2UlFypUcxJVZHqf8k0IGTlzsSlhuciaiXAg"
    );
    assert_eq!(h.device, "dev_1");
    assert_eq!(h.timestamp, "1760000000000");
}

#[test]
fn the_signed_text_follows_the_spec() {
    assert_eq!(
        signing_string("POST", PATH, 1_760_000_000_000, "ZGVm", BODY),
        "sb-v1\nPOST\n/api/cli/v1/tokens\n1760000000000\nZGVm\n\
         4e13d569d24b2aa0c7841b24af20a4841cdc118605865ae5e7e039f1b4ec83c5"
    );
    // an empty body still has a hash (sha256 of nothing)
    assert!(
        signing_string("GET", "/x", 1, "n", b"")
            .ends_with("\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
}

#[test]
fn every_part_of_the_request_changes_the_signature() {
    let base = |method, path, ts, nonce: &[u8; 16], body: &[u8]| {
        sign_request(&key(), &device(), method, path, ts, nonce, body).signature
    };
    let reference = base("POST", PATH, 5, &NONCE, BODY);
    let mut other_nonce = NONCE;
    other_nonce[0] ^= 1;
    assert_ne!(reference, base("GET", PATH, 5, &NONCE, BODY));
    assert_ne!(
        reference,
        base("POST", "/api/cli/v1/status", 5, &NONCE, BODY)
    );
    assert_ne!(reference, base("POST", PATH, 6, &NONCE, BODY));
    assert_ne!(reference, base("POST", PATH, 5, &other_nonce, BODY));
    assert_ne!(reference, base("POST", PATH, 5, &NONCE, b"{}"));
    assert_eq!(
        reference,
        base("POST", PATH, 5, &NONCE, BODY),
        "deterministic"
    );
}

#[test]
fn the_key_survives_a_round_trip_through_text() {
    let k = key();
    let text = k.to_text();
    assert_eq!(text.len(), 43);
    assert_eq!(
        DeviceKey::from_text(&text).unwrap().public_key(),
        k.public_key()
    );
    assert!(
        DeviceKey::from_text(&format!("{}\n", text.as_str())).is_some(),
        "a trailing newline is fine"
    );
}

#[test]
fn text_that_is_not_a_key_is_refused() {
    for bad in [
        "",
        "AAAA",
        "not base64!",
        &"A".repeat(44),
        &"A".repeat(42),
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8==",
    ] {
        assert!(DeviceKey::from_text(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn the_key_never_shows_up_in_debug_output() {
    let k = key();
    let secret = k.to_text().to_string();
    for shown in [
        format!("{k:?}"),
        format!("{k:#?}"),
        format!("{:?}", Some(&k)),
    ] {
        assert!(!shown.contains(&secret), "{shown}");
        assert!(shown.contains("redacted"));
    }
}

#[test]
fn a_device_id_is_safe_to_put_in_a_header() {
    for ok in ["a", "dev_1", "3f2b8c1e-0000-4000-8000-123456789abc"] {
        assert!(DeviceId::new(ok).is_some(), "{ok}");
    }
    for bad in [
        "",
        "a b",
        "a\r\nx-evil: 1",
        "é",
        &"a".repeat(65),
        "a/b",
        "a:b",
    ] {
        assert!(DeviceId::new(bad).is_none(), "{bad:?}");
    }
}
