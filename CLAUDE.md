# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## Project overview

`photoflow` is a single-binary Rust CLI for managing a personal photography library. It organizes originals into dated folders, trashes throwaway JPEGs, backs RAWs + videos up to Backblaze B2, and scaffolds export folders on demand.

See `PROJECT.md` for the full implementation specification. This file captures the principles and constraints that govern how code is written in this repo.

---

## Commands

```bash
cargo build                        # build
cargo run -- <subcommand>          # run in dev
cargo test                         # all tests
cargo test --test naming           # single test file
cargo test -- --include-ignored    # include B2 integration tests (#[ignore] by default)
cargo clippy -- -D warnings        # lint
```

---

## Core principles (non-negotiable)

### Idempotency above all

Every command must be safe to run twice. "Already done" is always success, never an error. Check-then-act everywhere. A second backup run must upload zero files. A second `month` run must print the path and exit 0. A second `clean --apply` run must trash nothing.

- `backup --dry-run` must show **what is not yet in B2** (files missing or size-mismatched). It changes nothing.
- `backup` must **never delete anything** from B2, under any circumstances.
- `clean` is **dry-run by default** — it only trashes files when `--apply` is passed.

### Dangerous operations require confirmation

Any operation that permanently affects user data (trashing files, uploading to B2 when credentials are present, etc.) must print a clear summary and prompt for confirmation unless the user explicitly passed `--apply` or equivalent. The tool must not be invasive.

### Originals tree is append-only

`photoflow` may create directories and trash JPEGs in the originals tree (`photos_root`). It must never rename, move, or reorganize existing files. Lightroom catalogs by path; moving files orphans the catalog.

### Deletions go to the OS trash, never `unlink`

Use the `trash` crate. Hard deletion is never acceptable.

---

## Configuration design

- **Most settings belong in the user config file** (`~/.config/photoflow/config.toml` on Linux, `~/Library/Application Support/photoflow/config.toml` on macOS), not in source code.
- Defaults are written by `photoflow init` and must be readable and writable from the CLI.
- **Never hardcode paths, bucket names, regions, extensions, or categories in logic**. These come from config.
- `~` in config paths must be expanded (use `shellexpand`).
- Config is loaded via `directories` crate; override with global `--config <path>`.

### Credentials

B2 credentials are never written to config or logged. Resolution order:
1. `PHOTOFLOW_B2_KEY_ID` / `PHOTOFLOW_B2_APP_KEY` environment variables.
2. OS keyring (`keyring` crate), service `photoflow`, accounts `b2_key_id` and `b2_app_key`.

`photoflow login` stores credentials in the keyring (idempotent — overwrites). If credentials are missing at `backup` time, print a clear message directing the user to run `photoflow login` or set the env vars, then exit non-zero.

---

## Constants and magic values

- **No magic strings or integers in logic.** Define constants at the appropriate scope:
  - Used in one module → `const` in that module.
  - Used across 2+ modules → `const` in a shared module (e.g. `fs_util.rs` or a `constants.rs`).
  - Project-wide or in the public API → top-level in `lib.rs` or a dedicated `constants` module.
- Extension lists (discard, backup), folder-naming patterns (`MM-Month`), slug rules, hidden-file names (`.DS_Store`, `._`, `Thumbs.db`) — all are constants, not inline strings.

---

## Architecture

```
src/
  main.rs          # clap dispatch only — no logic here
  config.rs        # load/write/default config, path expansion, credential resolution
  naming.rs        # month dir names ("06-June"), export slug, date formatting
  fs_util.rs       # walkdir wrapper, extension matching (case-insensitive), hidden-file filter, size helpers
  b2.rs            # S3 client construction, HEAD/upload helpers (trait-abstracted for testing)
  commands/
    init.rs        # write default config if absent; mkdir photos_root + exports_root
    login.rs       # prompt (rpassword) + store to keyring
    month.rs       # create YYYY/MM-Month dir
    clean.rs       # trash discard_extensions files (dry-run by default)
    backup.rs      # HEAD-then-skip upload loop with indicatif progress
    export.rs      # create exports_root/category/YYYY-MM-DD-slug dir
    status.rs      # read-only summary; --remote triggers HEAD checks
tests/
  naming.rs        # month-name + slug unit tests
  idempotency.rs   # month/export/clean re-run no-op tests (tempdir)
```

Only `backup`, `login`, and `status --remote` are async. Wrap S3 work in a small `tokio::runtime::Runtime::block_on` block; keep the rest of the CLI synchronous.

---

## Key behaviors

### File walking

- Case-insensitive extension matching everywhere (`.NEF`, `.nef`, `.JPG`, `.jpg` all match).
- Ignore dotfiles, `.DS_Store`, `Thumbs.db`, and macOS `._` AppleDouble files in all walks.
- Do not follow symlinks (avoid loops / double-processing).
- Treat unknown files as inert — never touch them unless they match a discard or backup extension.

### `backup` idempotency primitive

For each candidate file: `HEAD` the object. Skip if it exists **and** its size equals the local file size. Upload only if missing or size-mismatched. Do not re-hash files on every run.

Process files in a deterministic order (stable sort by key) so resumed runs are predictable.

### Naming

- Month dirs: `MM-Month` (e.g. `06-June`, `11-November`). Numeric prefix is required for chronological sort.
- Export slugs: lowercase, spaces/underscores → single hyphens, non-alphanumerics stripped.
- Export leaf dirs: `YYYY-MM-DD-<slug>`.

### `clean` sidecar rule

When trashing `FILE.<ext>`, also trash `FILE.<ext>.xmp` if `discard_sidecars = true` in config (default true).

---

## Error handling

- Binary: use `anyhow` for error propagation.
- Any lib boundary (trait, reusable module): use `thiserror` for typed errors.
- Use `tracing` + `tracing-subscriber` for logging, gated behind `-v/--verbose`.

---

## Git

Only run read-only git commands (`git diff`, `git log`, `git status`, `git show`, etc.). Never commit, push, pull, rebase, merge, reset, or otherwise mutate git state — the user owns all git operations.

---

## Out of scope

Do not build: camera access (PTP/MTP/USB), EXIF-based auto-routing, background daemon, card-insertion watcher, backup bucket for exports, RAW-sibling guard on `clean`.
