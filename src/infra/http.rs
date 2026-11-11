//! The HTTP client: HTTPS only (plain `http://` just for this machine), no redirects, a 15 s
//! timeout, and every authenticated request signed afresh.
use super::clock::{Clock, SystemClock};
use crate::api::{ApiFailure, ServerUrl};
use crate::auth::{DeviceId, DeviceKey, sign_request};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
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
        let mut request = ureq::http::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.server.base()));
        let bytes = body.unwrap_or_default();
        if !bytes.is_empty() {
            request = request.header("content-type", "application/json");
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
