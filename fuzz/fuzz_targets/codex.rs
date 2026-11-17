//! Any text as a Codex rollout: no panic and the same answer twice, read whole or resumed
//! after an ordinal.
#![no_main]
use libfuzzer_sys::fuzz_target;
use spendbranch::sources::codex::{ReadOptions, read};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let seen = |key: u64| key % 3 == 0;
    for options in [
        ReadOptions::default(),
        ReadOptions {
            after_ordinal: Some(2),
            seen: Some(&seen),
        },
    ] {
        let result = read(text, &options);
        assert_eq!(result, read(text, &options), "not deterministic");
    }
});
