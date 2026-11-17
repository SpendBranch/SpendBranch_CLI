//! Any text as a Claude Code transcript: no panic, the same answer twice, and every line it
//! could not use is a real line, counted once, in order.
#![no_main]
use libfuzzer_sys::fuzz_target;
use spendbranch::sources::claude_code::{ReadOptions, read};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let seen = |key: u64| key % 3 == 0;
    for options in [
        ReadOptions::default(),
        ReadOptions {
            subagent_file: true,
            seen: Some(&seen),
        },
    ] {
        let result = read(text, &options);
        assert_eq!(result, read(text, &options), "not deterministic");
        let lines = text.split('\n').count();
        assert!(result.unsupported_lines.windows(2).all(|w| w[0] < w[1]));
        assert!(
            result
                .unsupported_lines
                .iter()
                .all(|&n| n >= 1 && n <= lines)
        );
    }
});
