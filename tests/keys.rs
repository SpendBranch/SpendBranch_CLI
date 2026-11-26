//! Where the private key is kept. The system keychain itself is never touched here: the
//! keychain code runs against keyring-core's in-memory store, and `open` is only asked what
//! it picks when a key file already exists.
use keyring_core::api::CredentialStoreApi;
use spendbranch::auth::DeviceKey;
use spendbranch::infra::keys::{self, Backend, Error, KeyFiles, KeyStore, Keychain, Slot};
use std::fs;

fn key(n: u8) -> DeviceKey {
    DeviceKey::from_seed(&[n; 32])
}

fn same(a: &Option<DeviceKey>, b: &DeviceKey) -> bool {
    a.as_ref().is_some_and(|a| a.public_key() == b.public_key())
}

fn stores() -> Vec<(&'static str, Box<dyn KeyStore>, Option<tempfile::TempDir>)> {
    let dir = tempfile::tempdir().unwrap();
    let files = KeyFiles::new(&dir.path().join("data"));
    let mock = keyring_core::mock::Store::new().unwrap();
    vec![
        ("file", Box::new(files), Some(dir)),
        (
            "keychain",
            Box::new(Keychain::with_store(mock, "inst-1")),
            None,
        ),
    ]
}

#[test]
fn both_stores_keep_two_independent_slots() {
    for (name, store, _dir) in stores() {
        assert!(
            store.load(Slot::Active).unwrap().is_none(),
            "{name}: starts empty"
        );
        store.save(Slot::Active, &key(1)).unwrap();
        assert!(same(&store.load(Slot::Active).unwrap(), &key(1)), "{name}");
        assert!(
            store.load(Slot::Pending).unwrap().is_none(),
            "{name}: slots are apart"
        );
        store.save(Slot::Pending, &key(2)).unwrap();
        store.save(Slot::Active, &key(3)).unwrap();
        assert!(
            same(&store.load(Slot::Active).unwrap(), &key(3)),
            "{name}: replaced"
        );
        assert!(same(&store.load(Slot::Pending).unwrap(), &key(2)), "{name}");
        store.delete(Slot::Active).unwrap();
        store.delete(Slot::Active).unwrap(); // nothing to delete is fine
        assert!(store.load(Slot::Active).unwrap().is_none(), "{name}");
        assert!(store.load(Slot::Pending).unwrap().is_some(), "{name}");
    }
}

#[test]
fn the_backend_says_where_the_key_is() {
    let stores = stores();
    assert_eq!(stores[0].1.backend(), Backend::File);
    assert_eq!(stores[1].1.backend(), Backend::Keychain);
}

#[test]
fn keychains_of_two_installations_do_not_mix() {
    let mock = keyring_core::mock::Store::new().unwrap();
    let a = Keychain::with_store(mock.clone(), "inst-a");
    let b = Keychain::with_store(mock, "inst-b");
    a.save(Slot::Active, &key(1)).unwrap();
    assert!(b.load(Slot::Active).unwrap().is_none());
}

#[test]
fn something_that_is_not_a_key_is_corrupt_and_not_echoed() {
    let mock = keyring_core::mock::Store::new().unwrap();
    let keychain = Keychain::with_store(mock.clone(), "i");
    mock.build("spendbranch-cli", "i/active", None)
        .unwrap()
        .set_password("hunter2-not-a-key")
        .unwrap();
    let err = keychain.load(Slot::Active).unwrap_err();
    assert!(matches!(err, Error::Corrupt));
    assert!(!format!("{err} {err:?}").contains("hunter2"));

    let dir = tempfile::tempdir().unwrap();
    let files = KeyFiles::new(dir.path());
    files.save(Slot::Active, &key(1)).unwrap();
    fs::write(dir.path().join("device.key"), "hunter2-not-a-key\n").unwrap();
    set_private(&dir.path().join("device.key"));
    let err = files.load(Slot::Active).unwrap_err();
    assert!(matches!(err, Error::Corrupt));
    assert!(!format!("{err} {err:?}").contains("hunter2"));
}

fn set_private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(unix)]
#[test]
fn the_key_file_is_private_and_a_readable_one_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    let files = KeyFiles::new(&root);
    files.save(Slot::Active, &key(7)).unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o700, "a folder we create is private");
    let file = root.join("device.key");
    assert_eq!(mode(&file), 0o600);
    assert!(
        fs::read_dir(&root)
            .unwrap()
            .all(|e| e.unwrap().file_name() != "device.key.tmp"),
        "the temporary file is gone"
    );

    for loose in [0o640, 0o604, 0o644, 0o660] {
        fs::set_permissions(&file, fs::Permissions::from_mode(loose)).unwrap();
        let err = files.load(Slot::Active).unwrap_err();
        assert!(matches!(err, Error::Insecure(_)), "{loose:o}");
        let shown = format!("{err} {err:?}");
        assert!(
            !shown.contains(key(7).to_text().as_str()),
            "the key stays out of the error"
        );
    }
    // saving again puts it right
    files.save(Slot::Active, &key(8)).unwrap();
    assert_eq!(mode(&file), 0o600);
    assert!(same(&files.load(Slot::Active).unwrap(), &key(8)));
}

#[test]
fn saving_both_slots_at_once_never_mixes_the_keys() {
    let dir = tempfile::tempdir().unwrap();
    for round in 0..200u8 {
        let (a, b) = (key(round), key(round.wrapping_add(100)));
        std::thread::scope(|s| {
            s.spawn(|| KeyFiles::new(dir.path()).save(Slot::Active, &a).unwrap());
            s.spawn(|| KeyFiles::new(dir.path()).save(Slot::Pending, &b).unwrap());
        });
        let files = KeyFiles::new(dir.path());
        assert!(
            same(&files.load(Slot::Active).unwrap(), &a),
            "round {round}"
        );
        assert!(
            same(&files.load(Slot::Pending).unwrap(), &b),
            "round {round}"
        );
    }
}

#[test]
fn a_failed_save_leaves_no_key_behind() {
    let dir = tempfile::tempdir().unwrap();
    // a folder where the key file should go makes the final rename fail
    fs::create_dir(dir.path().join("device.key")).unwrap();
    assert!(
        KeyFiles::new(dir.path())
            .save(Slot::Active, &key(1))
            .is_err()
    );
    let left: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["device.key"], "no temporary copy of the key");
}

#[test]
fn a_leftover_temporary_file_does_not_leak_into_a_newer_key() {
    let dir = tempfile::tempdir().unwrap();
    // a crash left a long temporary file behind
    let temp = format!("device.key.active.{}.tmp", std::process::id());
    fs::write(dir.path().join(temp), "x".repeat(500)).unwrap();
    let files = KeyFiles::new(dir.path());
    files.save(Slot::Active, &key(4)).unwrap();
    assert!(same(&files.load(Slot::Active).unwrap(), &key(4)));
}

#[test]
fn a_key_file_that_is_already_there_wins_over_the_keychain() {
    let dir = tempfile::tempdir().unwrap();
    KeyFiles::new(dir.path())
        .save(Slot::Pending, &key(5))
        .unwrap();
    let store = keys::open(dir.path(), "inst-1", Some(dir.path()));
    assert_eq!(store.backend(), Backend::File);
    assert!(same(&store.load(Slot::Pending).unwrap(), &key(5)));
}

#[test]
fn new_keys_are_random() {
    let (a, b) = (keys::generate().unwrap(), keys::generate().unwrap());
    assert_ne!(a.public_key(), b.public_key());
    assert_eq!(a.public_key().len(), 43);
}

/// Touches the machine's real keychain (read-only probe), so it is run by hand:
/// `cargo test --test keys -- --ignored --nocapture`
#[test]
#[ignore = "uses the real system keychain"]
fn the_real_keychain_is_probed_without_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let store = keys::open(dir.path(), "probe-installation", Some(dir.path()));
    println!(
        "backend: {:?} after {:?}",
        store.backend(),
        started.elapsed()
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    if store.backend() == Backend::Keychain {
        // one real round trip, cleaned up at the end
        store.save(Slot::Pending, &key(9)).unwrap();
        assert!(same(&store.load(Slot::Pending).unwrap(), &key(9)));
        store.delete(Slot::Pending).unwrap();
        assert!(store.load(Slot::Pending).unwrap().is_none());
    }
}

#[test]
fn a_folder_is_inside_the_profile_only_on_the_same_local_path() {
    use keys::within_profile as within;
    let me = r"C:\Users\me";
    for inside in [
        r"C:\Users\me",
        r"C:\Users\me\AppData\Local\spendbranch",
        r"\\?\C:\Users\me\spendbranch",
        r"c:\users\ME\spendbranch\",
        "C:/Users/me/spendbranch",
    ] {
        assert!(within(inside, me), "{inside}");
    }
    assert!(within(r"\\?\C:\Users\me\x", r"\\?\C:\Users\me"));
    for outside in [
        r"C:\Users\meg\spendbranch",
        r"C:\Users",
        r"D:\Users\me\spendbranch",
        r"C:\spendbranch",
        r"\\server\share\Users\me\spendbranch",
        r"\\?\UNC\server\share\me",
        r"\\.\C:\Users\me\x",
        r"C:\Users\me\..\other",
        r"spendbranch",
        "",
    ] {
        assert!(!within(outside, me), "{outside}");
    }
    // A profile on the network counts as outside: nothing is inside it.
    assert!(!within(r"\\server\home\me\x", r"\\server\home\me"));
    assert!(!within(r"C:\x", ""));
}

#[cfg(windows)]
#[test]
fn on_windows_a_key_file_outside_the_profile_is_refused_and_nothing_is_written() {
    let profile = tempfile::tempdir().unwrap();
    let inside = profile.path().join("spendbranch");
    fs::create_dir_all(&inside).unwrap();
    let files = KeyFiles::in_profile(&inside, Some(profile.path()));
    files.save(Slot::Active, &key(1)).unwrap();
    assert!(same(&files.load(Slot::Active).unwrap(), &key(1)));

    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("spendbranch");
    fs::create_dir_all(&outside).unwrap();
    for profile in [Some(profile.path()), None] {
        let files = KeyFiles::in_profile(&outside, profile);
        let err = files.save(Slot::Active, &key(2)).unwrap_err();
        assert!(matches!(err, Error::OutsideProfile(_)), "{err:?}");
        let text = err.to_string();
        assert!(
            text.contains("SPENDBRANCH_HOME") && text.contains("Credential Manager"),
            "{text}"
        );
        assert!(!text.contains(key(2).to_text().as_str()));
        assert_eq!(
            fs::read_dir(&outside).unwrap().count(),
            0,
            "no key file and no temporary copy"
        );
    }
}

#[cfg(windows)]
#[test]
fn on_windows_a_key_file_already_outside_the_profile_is_not_read_and_is_left_in_place() {
    let profile = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("spendbranch");
    KeyFiles::new(&outside).save(Slot::Active, &key(3)).unwrap();
    for profile in [Some(profile.path()), None] {
        let files = KeyFiles::in_profile(&outside, profile);
        let err = files.load(Slot::Active).unwrap_err();
        assert!(matches!(err, Error::KeyOutsideProfile(_)), "{err:?}");
        let text = err.to_string();
        assert!(
            text.contains("outside your user profile")
                && text.contains("SPENDBRANCH_HOME")
                && text.contains("spendbranch login"),
            "{text}"
        );
        assert!(!text.contains(key(3).to_text().as_str()));
    }
    assert!(
        outside.join("device.key").exists(),
        "the file is not deleted"
    );
    // The guarded store picks the file (it is there) and refuses it the same way.
    let store = keys::open(&outside, "inst-outside", Some(profile.path()));
    assert_eq!(store.backend(), Backend::File);
    assert!(matches!(
        store.load(Slot::Active),
        Err(Error::KeyOutsideProfile(_))
    ));
}
