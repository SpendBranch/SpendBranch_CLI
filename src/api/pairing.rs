//! The pairing messages (device code flow with the device key).
use super::ServerUrl;
use crate::auth::DeviceId;
use crate::contract::Iso;
use serde::{Deserialize, Serialize};

/// `POST /api/cli/v1/device-codes`
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartPairing<'a> {
    pub installation_id: &'a str,
    pub cli_version: Option<&'a str>,
    /// The device's public key, base64url of 32 bytes.
    pub public_key: &'a str,
}

/// `POST /api/cli/v1/tokens`
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollToken<'a> {
    pub device_code: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStarted {
    /// The CLI's secret for polling.
    pub device_code: String,
    /// What the person sees, `BCDF-GHJK`.
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

const USER_CODE_ALPHABET: &str = "BCDFGHJKLMNPQRSTVWXZ";

impl PairingStarted {
    /// Accepts the answer only if every field is well formed and both links lead to the
    /// server we asked: a hostile or buggy server cannot point the browser elsewhere.
    pub fn checked(self, server: &ServerUrl) -> Option<Self> {
        let code_ok = (1..=64).contains(&self.device_code.len())
            && self
                .device_code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        let user_ok = self.user_code.len() == 9
            && self.user_code.char_indices().all(|(i, c)| {
                if i == 4 {
                    c == '-'
                } else {
                    USER_CODE_ALPHABET.contains(c)
                }
            });
        let timing_ok = (1..=3600).contains(&self.expires_in) && (1..=60).contains(&self.interval);
        // The links are printed, so nothing but visible ASCII: no escape sequence, no bidi mark.
        let plain = |link: &str| link.len() <= 2048 && link.bytes().all(|b| b.is_ascii_graphic());
        let links_ok = [&self.verification_uri, &self.verification_uri_complete]
            .into_iter()
            .all(|link| plain(link) && server.same_origin(link));
        (code_ok && user_ok && timing_ok && links_ok).then_some(self)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PollOutcome {
    AuthorizationPending,
    SlowDown,
    ExpiredToken,
    AccessDenied,
    #[serde(rename_all = "camelCase")]
    Approved {
        device_id: String,
        expires_at: String,
        organization_id: String,
        organization_name: String,
    },
}

/// A poll answer that passed validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Polled {
    Pending,
    SlowDown,
    Expired,
    Denied,
    Approved(Approval),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub device_id: DeviceId,
    pub expires_at: Iso,
    pub organization_id: String,
    /// Safe to print: no control or direction-changing characters.
    pub organization_name: String,
}

/// Text the server chose that we will print: no control or invisible-direction characters.
pub(super) fn printable(text: &str, max_chars: usize) -> bool {
    let n = text.chars().count();
    (1..=max_chars).contains(&n)
        && text.chars().all(|c| {
            !c.is_control()
                && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
        })
}

impl PollOutcome {
    pub fn checked(self) -> Option<Polled> {
        Some(match self {
            Self::AuthorizationPending => Polled::Pending,
            Self::SlowDown => Polled::SlowDown,
            Self::ExpiredToken => Polled::Expired,
            Self::AccessDenied => Polled::Denied,
            Self::Approved {
                device_id,
                expires_at,
                organization_id,
                organization_name,
            } => Polled::Approved(Approval {
                device_id: DeviceId::new(&device_id)?,
                expires_at: Iso::parse(&expires_at)?,
                organization_id: (printable(&organization_id, 64)).then_some(organization_id)?,
                organization_name: printable(&organization_name, 200)
                    .then_some(organization_name)?,
            }),
        })
    }
}
