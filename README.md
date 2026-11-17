# SpendBranch CLI

Collects only company-authorized metadata (IDs, model, token counts, normalized repository and branch) from local AI coding agent sessions and uploads it to SpendBranch. Prompts, responses, code and local paths never leave the device.

Status: pre-release.

## Install

Download a release and run `spendbranch setup`: see [docs/INSTALL.md](docs/INSTALL.md), which also
explains how to uninstall.

## License

This repository is **source-available for auditing, not open source**. It is public so anyone can see exactly what the CLI does before installing it. You may read the code, and build it unmodified to verify an official binary. Copying or reusing any part of it, commercial use, forks, modification and redistribution are not allowed, and the code may not be used to train or improve any AI or machine learning model. See [LICENSE](LICENSE).

## Develop

Requires Rust 1.89 or newer.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo deny check
```

- `schema/ingest-v1.json`: JSON Schema of the upload contract (`ingest/v1`).
- `tests/fixtures/`: synthetic fixture corpus; see its README.
- `cargo test --release --test perf -- --ignored --nocapture --test-threads=1`: performance budget.

## Release

Push a tag equal to the version in `Cargo.toml` (`git tag v0.2.0 && git push origin v0.2.0`).
`.github/workflows/release.yml` runs the whole CI on the 6 systems, builds the 6 binaries, and
publishes the release with the archives and `SHA256SUMS` (a tag with a hyphen, such as
`v0.2.0-rc.1`, is a pre-release).

- Production server: the repository variable `SPENDBRANCH_DEFAULT_SERVER` (https only) becomes
  the release's default. Without it, a release needs `--server` or `SPENDBRANCH_SERVER`.
- Signing runs only when its repository secrets exist; otherwise the binary ships unsigned, with
  a warning in the run.
  - macOS: `APPLE_CERTIFICATE_P12` (base64 of the Developer ID Application .p12),
    `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, and for notarization `APPLE_ID`,
    `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` (an app-specific password).
  - Windows: `WINDOWS_CERTIFICATE_PFX` (base64 of the .pfx) and `WINDOWS_CERTIFICATE_PASSWORD`.
- `fuzz/` holds the cargo-fuzz targets (`cargo +nightly fuzz run claude_code`); they run weekly
  in `.github/workflows/fuzz.yml`.

## Layout

The code is a functional core with an imperative shell, checked by `tests/architecture.rs`:

| Module | Role |
| --- | --- |
| `contract` | the `ingest/v1` types; an invalid value cannot be built or read |
| `auth`, `api` | the device key's signature on every request, the server address rules and the wire messages; no I/O |
| `sources` | pure parsers and coverage rules; no I/O |
| `readers` | find session files and read what is new; no database |
| `infra` | the local database, the sync lock, private files, the device key store, the HTTPS client |
| `cli`, `commands` | arguments, texts, and what each command does |

Pure modules never depend on `infra`, and `readers` never touch the database.

See [SECURITY.md](SECURITY.md) to report a vulnerability.
