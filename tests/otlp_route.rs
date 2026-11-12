//! The fake server's OTLP route against the contract: the strict filter, and the route's own
//! ceiling. The relay's filter never lets these cases through, so they are posted directly.
#[path = "common/fake_server.rs"]
mod fake_server;

use fake_server::FakeServer;
use serde_json::{Value, json};
use spendbranch::api::{ApiFailure, ServerUrl};
use spendbranch::auth::DeviceId;
use spendbranch::infra::{
    http::{Client, Signer},
    keys,
};

const LOGS: &str = "/api/ingest/otlp/v1/logs";

fn post(payload: &Value) -> Result<Value, ApiFailure> {
    let server = FakeServer::start();
    let key = keys::generate().unwrap();
    server.register("dev-1", &key.public_key());
    let id = DeviceId::new("dev-1").unwrap();
    let client = Client::new(ServerUrl::parse(&server.base).unwrap(), "1.0.0");
    client.post(
        LOGS,
        payload,
        Some(&Signer {
            device: &id,
            key: &key,
        }),
    )
}

fn export(attributes: Value) -> Value {
    json!({"resourceLogs": [{"scopeLogs": [{"logRecords": [
        {"eventName": "api_request", "timeUnixNano": "1767261601000000000", "attributes": attributes}
    ]}]}]})
}

#[test]
fn a_record_with_a_repeated_attribute_key_is_refused_whole() {
    let answer = post(&export(json!([
        {"key": "model", "value": {"stringValue": "a"}},
        {"key": "model", "value": {"stringValue": "b"}},
    ])))
    .unwrap();
    assert_eq!(answer["partialSuccess"]["rejectedLogRecords"], 1);
}

#[test]
fn a_record_with_every_key_once_is_taken() {
    let answer = post(&export(json!([
        {"key": "model", "value": {"stringValue": "a"}},
        {"key": "input_tokens", "value": {"intValue": 5}},
    ])))
    .unwrap();
    assert_eq!(answer, json!({}));
}

#[test]
fn the_route_has_a_ceiling_of_60_batches_a_minute_per_device() {
    let server = FakeServer::start();
    let key = keys::generate().unwrap();
    server.register("dev-1", &key.public_key());
    server.register("dev-2", &key.public_key());
    let client = Client::new(ServerUrl::parse(&server.base).unwrap(), "1.0.0");
    let send = |device: &str| {
        let id = DeviceId::new(device).unwrap();
        client.post::<_, Value>(
            LOGS,
            &export(json!([])),
            Some(&Signer {
                device: &id,
                key: &key,
            }),
        )
    };
    for _ in 0..60 {
        send("dev-1").unwrap();
    }
    let failure = send("dev-1").unwrap_err();
    assert_eq!(
        (failure.status, failure.code.as_str()),
        (429, "RATE_LIMITED")
    );
    assert!(
        failure
            .retry_after_secs
            .is_some_and(|s| (1..=60).contains(&s))
    );
    assert!(send("dev-2").is_ok(), "the ceiling is per device");
}
