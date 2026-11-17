//! The relay's filter walks the body as it is parsed: a 1 MiB body costs a small part of what
//! parsing it into a `Value` would. One test only: it reads a process-wide allocation counter.
use spendbranch::relay;

mod common;
use common::extra_peak;

#[global_allocator]
static ALLOC: common::Counting = common::Counting;

#[test]
fn a_1_mib_body_of_tiny_values_costs_a_small_part_of_its_parse() {
    let zeros = vec!["0"; 500_000].join(",");
    let body = format!(r#"{{"resourceLogs":[{{"scopeLogs":[{{"logRecords":[{zeros}]}}]}}]}}"#);
    assert!(body.len() <= relay::MAX_BODY as usize);
    let (parse, _) = extra_peak(|| serde_json::from_slice::<serde_json::Value>(body.as_bytes()));
    let (peak, prepared) = extra_peak(|| relay::prepare(body.as_bytes()));
    assert_eq!(prepared.ok().map(|p| p.refused), Some(500_000));
    assert!(
        peak < parse / 20 && peak < 2 * 1024 * 1024,
        "the filter used {peak} bytes; parsing alone takes {parse}"
    );
}
