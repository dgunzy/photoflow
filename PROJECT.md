# `photoflow` — implementation spec

A small, single-binary Rust CLI for managing a personal photography library: create the
month/year folders, clean throwaway JPEGs, back original RAWs + videos up to Backblaze B2,
and scaffold dated export folders on demand.

This document is the complete specification. Build it as described. Where a detail is left
open, prefer the **simplest** option and note the choice in a code comment.

---

## 1. Guiding principles (read first, they constrain everything)

1. **Idempotency is the top priority.** Every command must be safe to run repeatedly. Running
   `month` twice must not error or duplicate. Running `backup` twice must re-upload nothing.
   Running `clean` twice must be a no-op the second time. No command may corrupt or duplicate
   state on a re-run. When in doubt, check-then-act, and make "already done" a success, not an error.

2. **The originals tree is append-only and immutable.** The tool *creates directories* in it and
   *removes throwaway JPEGs* from it, but never renames, moves, or reorganizes originals. Lightroom
   catalogs these files by path; reorganizing them would orphan the catalog. The folder layout the
   tool writes is permanent.

3. **The exports tree is disposable.** It is regenerable from RAW + Lightroom, so it can be freely
   structured. Export dirs are created **on demand only**, never speculatively.

4. **Deletion is recoverable.** "Delete" means *move to the OS trash*, never a hard unlink. Use the
   `trash` crate. `clean` is **dry-run by default**; it only trashes files when `--apply` is passed.

5. **No secrets in config or code.** The only secret is the B2 application key. It comes from the OS
   keyring or an environment variable. Everything else (endpoint, region, bucket) lives in config.

---

## 2. Directory schema

Two separate trees with opposite rules.

```
~/Pictures/Photos/                  # ORIGINALS — precious, backed up, immutable
  2026/
    06-June/                        # numeric prefix => sorts chronologically forever
      DSC_0001.NEF
      DSC_0001.NEF.xmp              # Lightroom-written sidecar, rides along to backup
      DSC_0001.MOV
    07-July/

~/Pictures/Exports/                 # reproducible, NOT precious, freely structured
  instagram/
    2026-06-15-charlie-bean/        # one dir per post, created on demand
  portfolio/
  personal/
```

- Month directories are named `MM-Month`, e.g. `06-June`, `11-November`. The numeric prefix is
  required so directories sort chronologically rather than alphabetically.
- Year directories are 4-digit, e.g. `2026`.
- Export category dirs (`instagram`, `portfolio`, `personal`) are created lazily the first time
  something is exported into them.
- Export leaf dirs are named `YYYY-MM-DD-<slug>`, where `<slug>` is the user-supplied name
  lowercased with spaces/underscores collapsed to single hyphens and non-alphanumerics stripped.

---

## 3. Configuration

TOML, loaded from the platform config dir via the `directories` crate
(`~/.config/photoflow/config.toml` on Linux, `~/Library/Application Support/photoflow/config.toml`
on macOS). `photoflow init` writes a default config there if absent (and must NOT overwrite an
existing one — print its path and exit 0 instead).

Paths support `~` expansion (use the `shellexpand` crate or expand `~` manually against the home dir).

```toml
[paths]
photos_root  = "~/Pictures/Photos"
exports_root = "~/Pictures/Exports"

[clean]
# Files with these extensions (case-insensitive) are throwaway and get trashed by `clean`.
discard_extensions = ["jpg", "jpeg"]
# When trashing FILE.jpg, also trash a sibling FILE.jpg.xmp if present.
discard_sidecars   = true

[backup]
endpoint = "https://s3.ca-east-006.backblazeb2.com"
region   = "ca-east-006"
bucket   = "photo-backup-dgunzy"
# Only files with these extensions (case-insensitive) are uploaded. RAWs, videos, and the
# Lightroom XMP sidecars that carry the edits. Everything else stays local.
backup_extensions = ["nef", "dng", "mov", "mp4", "heic", "xmp"]
# Object keys are paths relative to photos_root, optionally under this prefix.
key_prefix = ""

[export]
# Recognized export categories. Free-form; these are just the ones scaffolded with nice defaults.
categories = ["instagram", "portfolio", "personal"]
```

### Credentials

The B2 application key (a B2 `keyID` + `applicationKey` pair, used as S3
access-key-id / secret-access-key) is resolved in this order:

1. Environment: `PHOTOFLOW_B2_KEY_ID` and `PHOTOFLOW_B2_APP_KEY`.
2. OS keyring (`keyring` crate), service `photoflow`, accounts `b2_key_id` and `b2_app_key`.

Provide a `photoflow login` command that prompts (hidden input via `rpassword`) for the key id and
app key and stores them in the keyring. If neither source yields credentials when `backup` runs,
print a clear message telling the user to run `photoflow login` or set the env vars, and exit
non-zero. Never write credentials to the config file or log them.

> In Backblaze, the key should be scoped to **only** the `photo-backup-dgunzy` bucket — not the
> master key. The tool does not enforce this; just document it in the README.

---

## 4. Commands

Use `clap` (derive API). Global `--config <path>` override and `-v/--verbose` flag. All commands
print a concise human summary; `clean` and `backup` also support `--json` for machine-readable output.

### `photoflow init`
Write the default config to the platform config path if it doesn't exist; otherwise print the
existing path and do nothing. Also ensure `photos_root` and `exports_root` exist (`mkdir -p`).

### `photoflow login`
Prompt for B2 key id + app key (hidden) and store in the keyring. Idempotent — overwrites prior
values. Confirm success without echoing the secret.

### `photoflow month [--date YYYY-MM]`
Create `<photos_root>/<YYYY>/<MM-Month>/`, creating the year dir as needed. Default date is the
current local month. Idempotent: if the dir exists, succeed and print its path. Print the absolute
path created/confirmed so the user can `cd` to it for the USB import.

### `photoflow clean [PATH] [--apply] [--json]`
Trash throwaway files (by `clean.discard_extensions`, case-insensitive) under `PATH`, defaulting to
`photos_root` if omitted. **Dry-run by default**: list what *would* be trashed and a total size, and
make NO changes. With `--apply`, move those files to the OS trash via the `trash` crate.

- If `discard_sidecars` is true, also trash `FILE.<ext>.xmp` when trashing `FILE.<ext>`.
- Recurse into subdirectories.
- Never touch files whose extension isn't in `discard_extensions` (RAWs, videos, HEIC, etc. are safe).
- Idempotent: a second run finds nothing to do.
- Per the owner's decision, no RAW-sibling guard is required — all JPEGs are throwaway. Because
  deletion goes to the trash, this is recoverable; state this plainly in `--help` text.

### `photoflow backup [--dry-run] [--json]`
Upload local originals to B2. **Must be fully idempotent.**

- Walk `photos_root`. Select files whose extension is in `backup.backup_extensions` (case-insensitive).
- Object key = `key_prefix` + path relative to `photos_root`, using forward slashes,
  e.g. `2026/06-June/DSC_0001.NEF`.
- For each candidate: `HEAD` the object. **Skip if it exists and its size equals the local size.**
  Upload only if missing or size-mismatched. (Size check is the cheap idempotency primitive; do not
  re-hash everything on every run.)
- `--dry-run` lists what would upload and the total bytes, uploads nothing.
- Use multipart upload for large files (videos). Show progress with `indicatif`.
- Print a summary: N uploaded, M skipped (already present), total bytes sent.
- Re-running immediately after a successful backup must upload zero files.

S3 client: use `aws-sdk-s3` (or `object_store`) with `endpoint_url` set to `backup.endpoint` and
`region` from config; path-style addressing as needed for B2's S3-compatible endpoint. Credentials
from §3.

### `photoflow export <category> <name> [--date YYYY-MM-DD]`
Create `<exports_root>/<category>/<YYYY-MM-DD>-<slug>/`, creating the category dir as needed. Default
date is today. `<category>` is free-form but warn (don't fail) if it's not in `export.categories`.
Print the absolute path. Idempotent: existing dir => succeed and print path.

### `photoflow status`
Read-only overview, no network unless `--remote` is passed:
- Per year/month: count of files, broken down by RAW / video / JPEG-still-present / xmp.
- Total library size.
- Count of JPEGs that `clean` would remove.
- With `--remote`: count and bytes of backup-eligible files not yet present in B2 (same HEAD/size
  logic as `backup --dry-run`, but just report).

---

## 5. Behavior details & edge cases

- **Case-insensitive extensions** everywhere (`.NEF`, `.nef`, `.JPG`, `.jpg` all match).
- **Hidden / system files**: ignore dotfiles, `.DS_Store`, `Thumbs.db`, and macOS `._` AppleDouble
  files in all walks; never upload or count them.
- **Symlinks**: do not follow symlinks when walking (avoid loops / double-processing).
- **Empty/partial month dirs** are fine; `month` creating an empty dir is expected.
- **Interrupted backup**: because idempotency is HEAD-then-skip, a re-run resumes cleanly. A file
  that was mid-upload (multipart aborted) will simply not exist server-side and re-upload.
- **`backup` ordering**: process smallest-first or stable-sorted by key — deterministic order makes
  resumed runs predictable. (Smallest-first gets quick wins on the progress bar; either is fine.)
- **No daemon, no filesystem watching, no card/PTP access.** The user copies from the camera over
  USB manually into the dir that `month` created. The tool never talks to the camera.
- Treat unknown/extra files in the tree as inert — leave them alone unless they match a discard or
  backup extension.

---

## 6. Suggested crates

| Concern              | Crate |
|----------------------|-------|
| CLI parsing          | `clap` (derive) |
| Config               | `serde`, `toml` |
| Platform paths       | `directories` |
| `~` expansion        | `shellexpand` |
| Directory walking    | `walkdir` |
| Trash (recoverable)  | `trash` |
| Keyring              | `keyring` |
| Hidden password in   | `rpassword` |
| S3 / B2 client       | `aws-sdk-s3` (with `aws-config`) or `object_store` |
| Async runtime        | `tokio` (for the S3 SDK) |
| Progress bars        | `indicatif` |
| Errors               | `anyhow` (binary), `thiserror` (any lib boundary) |
| Logging              | `tracing` + `tracing-subscriber` (gated by `-v`) |
| JSON output          | `serde_json` |

Only `backup`/`login`/`status --remote` need async; keep the rest synchronous and put the S3 work
behind a small async runtime block so the CLI stays simple.

---

## 7. Project layout

```
photoflow/
  Cargo.toml
  README.md                 # usage, B2 key setup, the immutability/idempotency rules
  src/
    main.rs                 # clap dispatch
    config.rs               # load/write/default config, path expansion, credential resolution
    commands/
      init.rs
      login.rs
      month.rs
      clean.rs
      backup.rs
      export.rs
      status.rs
    fs_util.rs              # walking, extension matching, hidden-file filtering, size helpers
    b2.rs                   # S3 client construction + HEAD/upload helpers
    naming.rs               # month dir names, export slug, date formatting
  tests/
    naming.rs               # month-name + slug unit tests
    idempotency.rs          # month/export/clean re-run no-op tests (use tempdir)
```

---

## 8. Tests (minimum)

- `naming`: `2026-06` → `06-June`; slug `"Charlie Bean!"` → `charlie-bean`; date formatting.
- `month` and `export` are no-ops on second run and return the same path.
- `clean` dry-run changes nothing; `clean --apply` against a tempdir trashes only discard-extension
  files and their sidecars, leaves RAWs/videos/HEIC untouched, and is a no-op on the second run.
- Backup idempotency can be unit-tested against a mocked HEAD (trait-abstract the S3 calls so a fake
  "already present, same size" implementation proves zero uploads on the second pass). A real B2
  integration test is optional and should be `#[ignore]`d by default.

---

## 9. README must document

- The two-tree model and the "originals are immutable, exports are disposable" rule.
- That `clean` trashes **all** JPEGs (recoverable via OS trash) and is dry-run by default.
- How to create a bucket-scoped B2 application key and run `photoflow login`.
- That Lightroom should **Add** photos in place (not import-copy) and have "automatically write
  changes to XMP" enabled, so edits live in `.xmp` sidecars that get backed up alongside the RAWs.
- That only RAWs + videos + XMP go to B2; exports stay local.

---

## 10. Explicitly out of scope (do not build)

- Talking to the camera (PTP/MTP/USB device access). Files arrive via manual USB copy.
- EXIF-based auto-routing of files into dated folders.
- A background daemon or card-insertion watcher.
- A second backup bucket for exports.
- Any RAW-sibling guard on `clean` — owner accepts that all JPEGs are throwaway.