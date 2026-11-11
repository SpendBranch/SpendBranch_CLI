//! The device's identity and the signature on every request (RFC 9449 style).
//!
//! There is no token: the server knows the device's public key, and each request carries a
//! signature only the private key can make. This module is pure: the clock and the random
//! nonce come in as arguments, so the same inputs always give the same signature.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::Zeroizing;

/// The server's name for a paired device. It goes into a header, so only plain characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(id: &str) -> Option<Self> {
        let ok = (1..=64).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        ok.then(|| Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The device's Ed25519 private key. It can sign and can be saved, but never printed: `Debug`
/// is redacted and there is no `Display`, `Clone` or `Serialize`.
pub struct DeviceKey(SigningKey);

impl DeviceKey {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    /// The text kept in the keychain or the key file: the 32-byte seed, base64url.
    pub fn to_text(&self) -> Zeroizing<String> {
        Zeroizing::new(B64.encode(self.0.as_bytes()))
    }

    /// Reads what `to_text` wrote. Anything else is `None`, and nothing is echoed.
    pub fn from_text(text: &str) -> Option<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        let n = B64
            .decode_slice(text.trim().as_bytes(), &mut seed[..])
            .ok()?;
        (n == 32).then(|| Self::from_seed(&seed))
    }

    /// The public half, base64url of 32 bytes: what the server stores to check signatures.
    pub fn public_key(&self) -> String {
        B64.encode(self.0.verifying_key().as_bytes())
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceKey(<redacted>)")
    }
}

/// The four `X-SB-*` headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedHeaders {
    pub device: String,
    pub timestamp: String,
    pub nonce: String,
    pub signature: String,
}

impl SignedHeaders {
    pub fn pairs(&self) -> [(&'static str, &str); 4] {
        [
            ("x-sb-device", &self.device),
            ("x-sb-timestamp", &self.timestamp),
            ("x-sb-nonce", &self.nonce),
            ("x-sb-signature", &self.signature),
        ]
    }
}

/// The exact text that is signed. `path` is the request path without the query, and
/// `body` is the bytes that go on the wire (after compression, if any).
pub fn signing_string(
    method: &str,
    path: &str,
    timestamp_ms: i64,
    nonce: &str,
    body: &[u8],
) -> String {
    let mut text = format!("sb-v1\n{method}\n{path}\n{timestamp_ms}\n{nonce}\n");
    for byte in Sha256::digest(body) {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Signs one attempt. A retry must call this again with a new `nonce` and `timestamp_ms`:
/// a repeated nonce makes the server treat the device as compromised.
pub fn sign_request(
    key: &DeviceKey,
    device: &DeviceId,
    method: &str,
    path: &str,
    timestamp_ms: i64,
    nonce: &[u8; 16],
    body: &[u8],
) -> SignedHeaders {
    let nonce = B64.encode(nonce);
    let text = signing_string(method, path, timestamp_ms, &nonce, body);
    SignedHeaders {
        device: device.as_str().to_owned(),
        timestamp: timestamp_ms.to_string(),
        signature: B64.encode(key.0.sign(text.as_bytes()).to_bytes()),
        nonce,
    }
}
