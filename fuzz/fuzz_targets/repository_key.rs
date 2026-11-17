//! Any text as a git remote: no panic, and a key is always `host/owner/name` in lower case,
//! at most 200 characters, with no credentials and no white space.
#![no_main]
use libfuzzer_sys::fuzz_target;
use spendbranch::sources::repository::repository_key_of;

fuzz_target!(|data: &[u8]| {
    let Ok(url) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(key) = repository_key_of(url) {
        assert!(key.len() <= 200, "{key}");
        assert!(
            !key.contains('@') && !key.contains(char::is_whitespace),
            "{key}"
        );
        assert_eq!(key, key.to_lowercase());
        assert!(
            key.split('/').filter(|p| !p.is_empty()).count() >= 3,
            "{key}"
        );
    }
});
