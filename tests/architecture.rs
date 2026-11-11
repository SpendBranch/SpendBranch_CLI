//! Dependency rule: the pure modules never touch the outside world.
//! `infra` is the only place for files, SQLite, locks, processes and the environment.
use std::{fs, path::Path};

/// Pure code: parsing, rules and texts. Add `src/contract` and `src/sync` as they appear.
const PURE: &[&str] = &[
    "src/sources",
    "src/cli/args.rs",
    "src/cli/messages.rs",
    "src/contract",
    "src/auth.rs",
    "src/api.rs",
    "src/sync",
];
const FORBIDDEN: &[&str] = &[
    "crate::infra",
    "crate::readers",
    "rusqlite",
    "std::fs",
    "std::env",
    "std::process",
    "std::net",
    "std::io::stdin",
];

fn rust_files(path: &Path, out: &mut Vec<std::path::PathBuf>) {
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            rust_files(&entry.unwrap().path(), out);
        }
    } else if path.extension().is_some_and(|e| e == "rs") {
        out.push(path.to_path_buf());
    }
}

#[test]
fn pure_modules_do_not_depend_on_infra_or_the_outside_world() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for p in PURE {
        let p = root.join(p);
        if p.exists() {
            rust_files(&p, &mut files);
        }
    }
    assert!(files.len() >= 3, "the pure modules went missing: {files:?}");
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for bad in FORBIDDEN {
            assert!(
                !text.contains(bad),
                "{} must stay pure but mentions `{bad}`",
                file.display()
            );
        }
    }
}

#[test]
fn infra_does_not_depend_on_the_command_layers() {
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/infra"),
        &mut files,
    );
    assert!(!files.is_empty());
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for bad in ["crate::cli", "crate::commands", "crate::readers"] {
            assert!(
                !text.contains(bad),
                "{} must not depend on `{bad}`",
                file.display()
            );
        }
    }
}

#[test]
fn readers_know_nothing_about_the_database_or_the_command_line() {
    // Readers only read files and return data; the store saves it. Keeping them apart is
    // what lets the commit be one transaction and lets readers be tested without SQLite.
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/readers"),
        &mut files,
    );
    assert!(files.len() >= 6, "the readers went missing: {files:?}");
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for bad in ["crate::infra", "rusqlite", "crate::cli", "crate::commands"] {
            assert!(
                !text.contains(bad),
                "{} must not depend on `{bad}`",
                file.display()
            );
        }
    }
}

#[test]
fn production_code_has_no_unsafe_and_no_panicking_shortcuts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for bad in [
            ".unwrap()",
            ".expect(",
            "panic!(",
            "unreachable!(",
            "todo!(",
            "unimplemented!(",
            "unsafe ",
        ] {
            assert!(!text.contains(bad), "{} uses `{bad}`", file.display());
        }
    }
}
