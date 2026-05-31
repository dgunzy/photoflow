# photoflow

A small, single-binary Rust CLI for managing a personal photography library. It creates
dated import folders, trashes throwaway JPEGs, backs original RAWs + videos + Lightroom
sidecars up to Backblaze B2, and scaffolds dated export folders on demand.

`photoflow` is built around one rule above all others: **every command is safe to run
twice.** "Already done" is success, never an error.

---

## The two-tree model

`photoflow` manages two separate directory trees with opposite rules.

```
~/Pictures/Photos/                 # ORIGINALS — precious, backed up, IMMUTABLE
  2026/
    06-June/                       # MM-Month: numeric prefix sorts chronologically
      DSC_0001.NEF
      DSC_0001.NEF.xmp             # Lightroom sidecar, rides along to the backup
      DSC_0001.MOV

~/Pictures/Exports/                # reproducible, NOT precious, freely structured
  instagram/
    2026-06-15-charlie-bean/       # one dir per post, created on demand
  portfolio/
  personal/
```

- **Originals are immutable.** `photoflow` only ever *creates directories* and *trashes
  throwaway JPEGs* in this tree. It never renames, moves, or reorganizes your files —
  Lightroom catalogs photos by path, so moving them would orphan the catalog.
- **Exports are disposable.** They are regenerable from RAW + Lightroom, so the tool
  creates export folders freely, but only **on demand** (never speculatively).

---

## Install

```bash
cargo build --release
# binary at target/release/photoflow
```

---

## Quick start

```bash
photoflow init                       # write default config, create the two roots
photoflow login                      # store B2 credentials in the OS keyring
photoflow month                      # mkdir this month's import folder, print its path
# ... copy photos off the card into that folder, edit in Lightroom ...
photoflow clean                      # preview which JPEGs would be trashed (dry-run)
photoflow clean --apply              # actually move them to the OS trash
photoflow backup                     # upload new RAWs/videos/xmp to B2 (idempotent)
photoflow export instagram "Charlie Bean"   # scaffold an export folder
photoflow status                     # read-only library overview
```

Global flags: `--config <path>` to use a non-default config file, `-v/--verbose` for
debug logging.

---

## Commands

| Command | What it does |
|---|---|
| `init` | Write the default config if absent (never overwrites), then `mkdir -p` the photos and exports roots. |
| `login` | Prompt (hidden) for the B2 key id + app key and store them in the OS keyring. Idempotent. |
| `month [--date YYYY-MM]` | Create `<photos_root>/<YYYY>/<MM-Month>/`. Defaults to the current month. Prints the path. |
| `clean [PATH] [--apply] [--json]` | Trash throwaway JPEGs (and `.xmp` sidecars). **Dry-run by default.** |
| `backup [--dry-run] [--json] [--yes]` | Upload originals to B2. Fully idempotent (HEAD-then-skip). |
| `export <category> <name> [--date YYYY-MM-DD]` | Create `<exports_root>/<category>/<YYYY-MM-DD>-<slug>/`. |
| `status [--remote]` | Read-only overview. `--remote` also reports what is not yet in B2. |

### `clean` trashes **all** JPEGs

`clean` treats **every** JPEG under the target path as throwaway — there is no
RAW-sibling guard. This is intentional: the workflow keeps RAWs and exports JPEGs from
Lightroom on demand, so in-tree JPEGs are camera throwaways.

This is safe because **deletion always goes to the OS trash** (via the `trash` crate),
never a hard `unlink` — it is fully recoverable. `clean` is **dry-run by default**; it
only moves files when you pass `--apply`. When it trashes `FILE.jpg`, it also trashes a
sibling `FILE.jpg.xmp` if present (`discard_sidecars`, on by default).

### `backup` is idempotent and append-only

For each eligible file, `backup` does a `HEAD` on the object and **skips it if it exists
with the same size**; it uploads only files that are missing or size-mismatched. Re-running
immediately after a successful backup uploads zero files. `backup` **never deletes anything
from B2**, under any circumstances. Large files (videos) use multipart upload.

`backup --dry-run` reports exactly what is not yet in B2 and changes nothing.

---

## Configuration

Config is TOML at the platform location:

- Linux: `~/.config/photoflow/config.toml`
- macOS: `~/Library/Application Support/photoflow/config.toml`

`photoflow init` writes the defaults (and never overwrites an existing file). Paths support
`~` expansion. Override the location with `--config <path>`.

```toml
[paths]
photos_root  = "~/Pictures/Photos"
exports_root = "~/Pictures/Exports"

[clean]
discard_extensions = ["jpg", "jpeg"]   # case-insensitive
discard_sidecars   = true              # also trash FILE.jpg.xmp

[backup]
endpoint = "https://s3.ca-east-006.backblazeb2.com"
region   = "ca-east-006"
bucket   = "photo-backup-dgunzy"
backup_extensions = ["nef", "dng", "mov", "mp4", "heic", "xmp"]
key_prefix = ""                        # object key = key_prefix + path relative to photos_root

[export]
categories = ["instagram", "portfolio", "personal"]
```

Object keys mirror the local layout, e.g. `2026/06-June/DSC_0001.NEF`. Only RAWs, videos,
and the Lightroom `.xmp` sidecars go to B2 — **exports stay local.**

---

## Backblaze B2 setup

1. Create a bucket (e.g. `photo-backup-dgunzy`) in your B2 account.
2. Create an **application key scoped to only that bucket** — *not* the master key. In the
   B2 console: *App Keys → Add a New Application Key*, restrict it to the one bucket.
   `photoflow` does not enforce the scoping; it is up to you to limit the key's reach.
3. Note the `keyID` and `applicationKey` B2 shows you (the app key is shown only once).
4. Run `photoflow login` and paste them in (input is hidden), or set the environment
   variables below.

### Credentials

The B2 key is the only secret, and it is **never written to config or logged.** It is
resolved in this order:

1. Environment: `PHOTOFLOW_B2_KEY_ID` and `PHOTOFLOW_B2_APP_KEY`.
2. OS keyring (service `photoflow`, accounts `b2_key_id` / `b2_app_key`), populated by
   `photoflow login`.

If neither yields credentials at `backup` time, the tool prints how to fix it and exits
non-zero.

---

## Lightroom workflow

For the backup to capture your edits, configure Lightroom so the edits live in `.xmp`
sidecars next to the RAWs:

- **Add** photos in place — do **not** use import-copy/move. The originals must stay where
  `photoflow month` put them, so the catalog path stays stable.
- Enable *Catalog Settings → Metadata → **Automatically write changes into XMP***. Edits
  then land in `FILE.NEF.xmp` sidecars, which `backup` uploads alongside the RAWs.

Only RAWs + videos + XMP are backed up. Exported JPEGs are reproducible and stay local.

---

## Development

```bash
cargo build
cargo test                         # all tests
cargo test --test naming           # a single test file
cargo test -- --include-ignored    # include any #[ignore]'d B2 integration tests
cargo clippy --all-targets -- -D warnings
```

The crate is a library (`src/lib.rs`) plus a thin clap dispatch binary (`src/main.rs`).
The network and trash boundaries are trait-abstracted (`RemoteStore`, `Trasher`) so the
idempotency logic is tested without hitting B2 or the real OS trash.
