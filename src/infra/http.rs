//! The HTTP client: HTTPS only (plain `http://` just for this machine), no redirects, a 15 s
//! timeout, and every authenticated request signed afresh.
use super::clock::{Clock, SystemClock};
use crate::api::{ApiFailure, ServerUrl};
use crate::auth::{DeviceId, DeviceKey, sign_request};
use flate2::{Compression, write::GzEncoder};
use serde::{Serialize, de::DeserializeOwned};
use std::{io::Write, sync::Arc, time::Duration};
use ureq::{
    Agent,
    config::Config,
    tls::{RootCerts, TlsConfig, TlsProvider},
};

/// An answer bigger than this is not ours: the API's replies are small.
const MAX_RESPONSE: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);

/// Who signs: the paired device and its key.
pub struct Signer<'a> {
    pub device: &'a DeviceId,
    pub key: &'a DeviceKey,
}

/// The crypto provider with its TLS 1.2 suites taken out. With none left, rustls does not even
/// offer 1.2 in the hello, so a server that speaks only 1.2 is refused while negotiating.
fn tls13_only() -> Arc<rustls::crypto::CryptoProvider> {
    let mut provider = rustls::crypto::ring::default_provider();
    provider
        .cipher_suites
        .retain(|suite| matches!(suite, rustls::SupportedCipherSuite::Tls13(_)));
    Arc::new(provider)
}

pub struct Client {
    agent: Agent,
    server: ServerUrl,
    clock: Box<dyn Clock>,
}

impl Client {
    pub fn new(server: ServerUrl, cli_version: &str) -> Self {
        Self::with_clock(server, cli_version, Box::new(SystemClock))
    }

    pub fn with_clock(server: ServerUrl, cli_version: &str, clock: Box<dyn Clock>) -> Self {
        let config = Config::builder()
            .timeout_global(Some(TIMEOUT))
            // A redirect could carry the signed request to another host: never follow one.
            .max_redirects(0)
            // Errors are read from the body, not raised by the status line.
            .http_status_as_error(false)
            // Belt and braces: `ServerUrl` already refuses plain http to a remote host.
            .https_only(!server.is_loopback())
            .user_agent(format!("spendbranch-cli/{cli_version}"))
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::Rustls)
                    // TLS 1.3 only: a provider with no older suites cannot negotiate an older version.
                    .unversioned_rustls_crypto_provider(tls13_only())
                    // The system's trusted roots, so a company's own CA keeps working.
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            );
        // A proxy variable in the environment must not capture a request to this machine.
        let config = if server.is_loopback() {
            config.proxy(None)
        } else {
            config
        };
        Self {
            agent: Agent::new_with_config(config.build()),
            server,
            clock,
        }
    }

    pub fn server(&self) -> &ServerUrl {
        &self.server
    }

    /// `POST` with a JSON body; the answer is read as `R`.
    pub fn post<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        signer: Option<&Signer>,
    ) -> Result<R, ApiFailure> {
        let bytes = serde_json::to_vec(body).map_err(|_| ApiFailure::plain(0, "INTERNAL"))?;
        self.json(self.send("POST", path, Some(bytes), signer)?)
    }

    pub fn get<R: DeserializeOwned>(
        &self,
        path: &str,
        signer: Option<&Signer>,
    ) -> Result<R, ApiFailure> {
        self.json(self.send("GET", path, None, signer)?)
    }

    /// `POST` of JSON that is sent gzipped. The signature covers the compressed bytes, the
    /// ones that go on the wire.
    pub fn post_json_gzip<R: DeserializeOwned>(
        &self,
        path: &str,
        json: &[u8],
        signer: Option<&Signer>,
    ) -> Result<R, ApiFailure> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        let compressed = gz
            .write_all(json)
            .and_then(|()| gz.finish())
            .map_err(|_| ApiFailure::plain(0, "INTERNAL"))?;
        self.json(self.send_with("POST", path, Some(compressed), signer, true)?)
    }

    /// `POST` with no body, answered `204`.
    pub fn post_empty(&self, path: &str, signer: Option<&Signer>) -> Result<(), ApiFailure> {
        self.send("POST", path, None, signer).map(|_| ())
    }

    fn json<R: DeserializeOwned>(&self, (status, body): (u16, Vec<u8>)) -> Result<R, ApiFailure> {
        serde_json::from_slice(&body).map_err(|_| ApiFailure::bad_response(status))
    }

    /// One attempt. Calling it again signs again, with a new time and a new nonce.
    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        signer: Option<&Signer>,
    ) -> Result<(u16, Vec<u8>), ApiFailure> {
        self.send_with(method, path, body, signer, false)
    }

    fn send_with(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        signer: Option<&Signer>,
        gzip: bool,
    ) -> Result<(u16, Vec<u8>), ApiFailure> {
        let mut request = ureq::http::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.server.base()));
        let bytes = body.unwrap_or_default();
        if !bytes.is_empty() {
            request = request.header("content-type", "application/json");
        }
        if gzip {
            request = request.header("content-encoding", "gzip");
        }
        if let Some(s) = signer {
            let mut nonce = [0u8; 16];
            getrandom::fill(&mut nonce).map_err(|_| ApiFailure::plain(0, "NO_ENTROPY"))?;
            let headers = sign_request(
                s.key,
                s.device,
                method,
                path,
                self.clock.now_ms(),
                &nonce,
                &bytes,
            );
            for (name, value) in headers.pairs() {
                request = request.header(name, value);
            }
        }
        let request = request.body(bytes).map_err(|_| ApiFailure::network())?;
        let response = self.agent.run(request).map_err(|_| ApiFailure::network())?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let (retry_after, server_time) = (header("retry-after"), header("x-sb-server-time"));
        let body = response
            .into_body()
            .with_config()
            .limit(MAX_RESPONSE)
            .read_to_vec()
            .map_err(|_| ApiFailure::network())?;
        if (200..300).contains(&status) {
            Ok((status, body))
        } else {
            Err(ApiFailure::from_response(
                status,
                &body,
                retry_after.as_deref(),
                server_time.as_deref(),
            ))
        }
    }
}
