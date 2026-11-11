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
