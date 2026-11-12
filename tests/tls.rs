//! The client speaks TLS 1.3 and nothing older. The test servers are real rustls servers on a
//! local port; what they report about the handshake is the proof, not the client's own word.
use rustls::{ServerConfig, ServerConnection, SupportedProtocolVersion, pki_types::PrivateKeyDer};
use spendbranch::api::ServerUrl;
use spendbranch::infra::http::Client;
use std::{net::TcpListener, sync::Arc, thread};

/// What the server saw of one handshake.
struct Handshake {
    /// The version it chose, if it got as far as choosing one.
    version: Option<rustls::ProtocolVersion>,
    error: Option<rustls::Error>,
}

/// Serves one TLS connection with the given versions; returns the address and the report.
fn serve_once(
    versions: &[&'static SupportedProtocolVersion],
) -> (u16, thread::JoinHandle<Handshake>) {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let config = ServerConfig::builder_with_protocol_versions(versions)
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into()),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let report = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut conn = ServerConnection::new(Arc::new(config)).unwrap();
        let mut error = None;
        while conn.is_handshaking() {
            if let Err(e) = conn.complete_io(&mut socket) {
                error = e
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                    .cloned();
                break;
            }
        }
        Handshake {
            version: conn.protocol_version(),
            error,
        }
    });
    (port, report)
}

fn call(port: u16) -> Result<serde_json::Value, spendbranch::api::ApiFailure> {
    let url = ServerUrl::parse(&format!("https://127.0.0.1:{port}")).unwrap();
    Client::new(url, "1.2.3").get("/api/cli/v1/status", None)
}

#[test]
fn a_server_that_speaks_only_tls_1_2_is_refused_while_negotiating() {
    let (port, report) = serve_once(&[&rustls::version::TLS12]);
    assert_eq!(call(port).unwrap_err().code, "NETWORK");
    let seen = report.join().unwrap();
    assert!(
        matches!(
            seen.error,
            Some(rustls::Error::PeerIncompatible(
                rustls::PeerIncompatible::Tls12NotOfferedOrEnabled
            ))
        ),
        "the client must not even offer 1.2: {:?}",
        seen.error
    );
    assert_eq!(seen.version, None, "no version was agreed");
}

#[test]
fn a_server_that_speaks_both_gets_tls_1_3() {
    let (port, report) = serve_once(rustls::ALL_VERSIONS);
    // the throwaway certificate is not trusted, so the call fails later: that is not the point
    assert!(call(port).is_err());
    let seen = report.join().unwrap();
    assert_eq!(seen.version, Some(rustls::ProtocolVersion::TLSv1_3));
}
