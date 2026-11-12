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

/// `tests/vectors/sb-v1.json` was computed by Python's `cryptography` (OpenSSL), not by this
/// crate, and the API copies the same file: if the signed text or the encoding drifts from
/// the spec, this is what notices.
#[test]
fn signature_matches_the_shared_vectors() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use sha2::{Digest, Sha256};
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/sb-v1.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let text = |x: &serde_json::Value| x.as_str().unwrap().to_owned();
    let seed: [u8; 32] = std::array::from_fn(|i| {
        u8::from_str_radix(&text(&v["seedHex"])[2 * i..2 * i + 2], 16).unwrap()
    });
    let key = DeviceKey::from_seed(&seed);
    assert_eq!(key.public_key(), text(&v["publicKey"]));
    // the fingerprint rule: sha256 of the 32 raw bytes, not of the base64url text
    let hex: String = Sha256::digest(B64.decode(key.public_key()).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let fingerprint = (0..4)
        .map(|i| &hex[4 * i..4 * i + 4])
        .collect::<Vec<_>>()
        .join("-");
    assert_eq!(fingerprint, text(&v["keyFingerprint"]));
    assert_eq!(
        key.fingerprint(),
        text(&v["keyFingerprint"]),
        "the crate's own function agrees"
    );
    assert_eq!(key.fingerprint(), "5647-5aa7-5463-474c");
    for c in v["cases"].as_array().unwrap() {
        let nonce: [u8; 16] = B64.decode(text(&c["nonce"])).unwrap().try_into().unwrap();
        let (method, path, body) = (text(&c["method"]), text(&c["path"]), text(&c["body"]));
        let ts = c["timestampMs"].as_i64().unwrap();
        let id = DeviceId::new(&text(&c["deviceId"])).unwrap();
        let h = sign_request(&key, &id, &method, &path, ts, &nonce, body.as_bytes());
        assert_eq!(h.signature, text(&c["signature"]), "{}", c["name"]);
        assert_eq!(h.nonce, text(&c["nonce"]));
        assert_eq!(h.timestamp, ts.to_string());
        assert_eq!(
            signing_string(&method, &path, ts, &h.nonce, body.as_bytes()),
            text(&c["signedString"])
        );
    }
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
