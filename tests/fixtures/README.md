# Fixture corpus

Synthetic data only: never copy real transcripts, accounts, paths or tokens here.

| Directory | Case layout | What `expected.json` holds |
| --- | --- | --- |
| `claude-code/<case>/` | `input.jsonl`, optional `options.json` (`role`, `seen`) | parser result: `observations`, `newKeys`, `unsupportedLines`, `untestedVersion`, `format` |
| `codex/<case>/` | `input.jsonl`, optional `options.json` (`afterOrdinal`, `seen`) | the same, plus `threadId`, `sessionId`, `lastOrdinal`, `foreignRecords` |
| `otlp/<case>/` | `input.json`, optional `options.json` (`strict`) | `observations`, `rejected`, `total`, and `relayPayload` (the lenient, rebuilt payload) |
| `repository/cases.json` | list of `{ input, expected }` | `repositoryKeyOf` results |

Every parser implementation must produce exactly these outputs. `cargo test` checks that each case is complete, that every expected observation conforms to `schema/ingest-v1.json`, and that nothing from the synthetic prompts, secrets, paths or e-mails leaks into an output.

Parser behaviour is covered at text level. Reader-level cases (files growing, rewritten, truncated, archived as `.zst`) need real files and belong with the readers.
