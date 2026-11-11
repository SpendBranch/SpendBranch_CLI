# Security

This repository is public so that anyone can read exactly what the SpendBranch CLI does on
their machine. If you find a way it could do more than that, or a way to abuse it, please
tell us before telling the world.

## Reporting a vulnerability

Use GitHub's private reporting: open the **Security** tab of this repository and choose
**Report a vulnerability**. Please do not open a public issue for a security problem.

Include what you found, how to reproduce it, and what you think the impact is. We will
acknowledge your report and keep you informed while we fix it.

## What the CLI is designed to guarantee

These are the properties worth trying to break:

- **Only metadata leaves the machine.** IDs, model, token counts, a normalized repository
  (`host/owner/name`) and a branch. Never prompts, responses, code, local paths, e-mail
  addresses or account names. The set of fields is fixed in the code and in
  `schema/ingest-v1.json`.
- **Nothing is trusted from the transcripts.** They are read as untrusted text: every field
  is validated against the contract before it is kept, and everything else is skipped without
  being stored.
- **Private by default.** The data folder is created `0700` and its files `0600` (on Windows
  the folder's ACL protects them). A folder anybody can write to is refused.
- **No shell, ever.** Reading your sessions starts no program except `git`, with fixed
  arguments, a 2-second timeout and no shell, to learn a project's remote. Folder names that
  come from a transcript are never interpreted, and network paths are never contacted.
- **The code does not need to be secret.** Security must not depend on anyone not having read
  this source. The only secret is a device's own private key, which never leaves its machine.

## Supported versions

Only the latest release is supported.
