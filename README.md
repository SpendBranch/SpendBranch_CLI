# SpendBranch CLI

Collects only company-authorized metadata (IDs, model, token counts, normalized repository and branch) from local AI coding agent sessions and uploads it to SpendBranch. Prompts, responses, code and local paths never leave the device.

Status: pre-release.

## Install

One command installs the CLI and connects this computer (`spendbranch setup`):

```sh
# macOS and Linux
curl -fsSL https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.ps1 | iex
```

Or `brew install spendbranch/tap/spendbranch`, or `winget install SpendBranch.CLI`. The script checks
the SHA-256 of what it downloads and needs no sudo or admin. See [docs/INSTALL.md](docs/INSTALL.md)
for the options, how to verify a release, installing by hand, and how to uninstall.

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
`.github/workflows/release.yml` runs the whole CI on the 6 systems, builds the 6 binaries (Linux
static, against musl), and publishes the release with the archives, `install.sh` and
`install.ps1` (with the version written in), `SHA256SUMS` and a build provenance attestation (a tag
with a hyphen, such as `v0.2.0-rc.1`, is a pre-release).

- Package managers, for releases only (never a pre-release); each is skipped, with a warning, while
  its secret is missing:
  - Homebrew: `HOMEBREW_TAP_TOKEN` (fine-grained, contents: write on `SpendBranch/homebrew-tap`).
    The job writes `Formula/spendbranch.rb` with `packaging/homebrew/formula.sh`.
  - winget: `WINGET_TOKEN` (classic, `public_repo`, of the account with the `winget-pkgs` fork).
    The first version is submitted by hand with the manifests from `packaging/winget/manifests.sh`;
    the job submits the later ones with komac.
- `install/`: the install scripts; `.github/scripts/installer-test.{sh,ps1}` test them end to end.

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
