# SpendBranch CLI

Collects only company-authorized metadata (IDs, model, token counts, normalized repository and branch) from local AI coding agent sessions and uploads it to SpendBranch. Prompts, responses, code and local paths never leave the device.

Status: under development (foundation phase).

## License

This repository is **source-available for auditing, not open source**. It is public so anyone can see exactly what the CLI does before installing it. You may read the code, and build it unmodified to verify an official binary. Commercial use, forks, modification and redistribution are not allowed. See [LICENSE](LICENSE).

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
- `cargo test --release --test perf -- --ignored --nocapture`: performance budget.

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
