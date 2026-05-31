//! `photoflow backup [--dry-run] [--json] [--yes]` — upload local originals to B2.
//!
//! Idempotency primitive: for each candidate, `HEAD` the object and skip it if it exists
//! with the same size; upload only if missing or size-mismatched. The plan/upload split
//! keeps the network seam ([`RemoteStore`]) testable and lets `--dry-run` and the
//! confirmation prompt reuse the exact same planning logic. `backup` never deletes.
//!
//! The planning phase runs HEAD checks concurrently (otherwise hundreds of sequential
//! round-trips feel like a hang), and the whole upload races against `Ctrl-C` so an
//! accidental sync can be aborted — already-uploaded files stay put and a re-run resumes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Context};
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;

use crate::b2::{B2Error, B2Store, RemoteStore};
use crate::config::{self, Config, ENV_APP_KEY, ENV_KEY_ID};
use crate::fs_util;

/// How many HEAD checks to keep in flight while planning. B2's S3 endpoint handles this
/// comfortably and it turns a multi-minute serial scan into a few seconds.
const HEAD_CONCURRENCY: usize = 16;
/// How often the progress spinners/bars repaint, so they stay alive during a long upload.
const TICK_INTERVAL: Duration = Duration::from_millis(120);
/// Default per-file retry policy for uploads. A single transient B2/network error no longer
/// aborts a long sync; we back off exponentially and try again.
const UPLOAD_RETRY_ATTEMPTS: u32 = 4;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

/// Exponential-backoff retry policy for a single upload.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// How many times to retry *after* the first attempt (0 = no retries).
    pub attempts: u32,
    /// Delay before the first retry; doubles each subsequent retry up to `max`.
    pub base: Duration,
    /// Ceiling on the backoff delay.
    pub max: Duration,
}

impl RetryPolicy {
    /// The production policy used by `backup`.
    pub fn standard() -> Self {
        Self {
            attempts: UPLOAD_RETRY_ATTEMPTS,
            base: RETRY_BASE_DELAY,
            max: RETRY_MAX_DELAY,
        }
    }

    /// No retries (used by tests that assert a single attempt).
    pub fn none() -> Self {
        Self {
            attempts: 0,
            base: Duration::ZERO,
            max: Duration::ZERO,
        }
    }

    fn delay_for(&self, attempt: u32) -> Duration {
        // Cap the shift so it can't overflow, then saturate against `max`.
        let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.max)
    }
}

/// A local file eligible for backup, paired with its remote object key.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub key: String,
    pub path: PathBuf,
    pub size: u64,
}

/// Outcome of comparing the local tree against the remote: what still needs uploading and
/// how many files are already present.
#[derive(Debug, Default)]
pub struct Plan {
    pub to_upload: Vec<Candidate>,
    pub skipped: usize,
}

impl Plan {
    pub fn pending_bytes(&self) -> u64 {
        self.to_upload.iter().map(|c| c.size).sum()
    }
}

/// Live upload progress, shared so the totals survive even if the upload future is dropped
/// on `Ctrl-C` (the atomics are updated as each file completes).
#[derive(Default)]
pub struct Progress {
    bar: Option<ProgressBar>,
    files_done: AtomicUsize,
    bytes_done: AtomicU64,
}

impl Progress {
    /// A no-op progress sink (used by tests).
    pub fn silent() -> Self {
        Self::default()
    }

    fn with_bar(bar: ProgressBar) -> Self {
        Self {
            bar: Some(bar),
            ..Self::default()
        }
    }

    fn record(&self, size: u64) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.bytes_done.fetch_add(size, Ordering::Relaxed);
        if let Some(bar) = &self.bar {
            bar.inc(size);
        }
    }

    pub fn files_done(&self) -> usize {
        self.files_done.load(Ordering::Relaxed)
    }

    pub fn bytes_done(&self) -> u64 {
        self.bytes_done.load(Ordering::Relaxed)
    }

    fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

/// Build the object key for `file`: `key_prefix` + path relative to `photos_root`, joined
/// with forward slashes.
pub fn object_key(photos_root: &Path, file: &Path, key_prefix: &str) -> anyhow::Result<String> {
    let rel = file
        .strip_prefix(photos_root)
        .with_context(|| format!("{} is not under photos_root", file.display()))?;
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join(fs_util::KEY_SEPARATOR);

    let prefix = key_prefix.trim_matches('/');
    Ok(if prefix.is_empty() {
        rel
    } else {
        format!("{prefix}{}{rel}", fs_util::KEY_SEPARATOR)
    })
}

/// True if `path` should be backed up: its extension is in `backup_extensions` and not in
/// `skip_extensions` (skip wins on overlap). Both checks are case-insensitive.
pub fn is_eligible(path: &Path, backup_extensions: &[String], skip_extensions: &[String]) -> bool {
    fs_util::has_extension(path, backup_extensions)
        && !fs_util::has_extension(path, skip_extensions)
}

/// Walk `photos_root` and collect every backup-eligible file, sorted by key for
/// deterministic (resumable) processing order.
pub fn gather_candidates(cfg: &Config) -> anyhow::Result<Vec<Candidate>> {
    let root = cfg.photos_root();
    let mut candidates = Vec::new();
    for path in fs_util::walk_files(&root) {
        if is_eligible(
            &path,
            &cfg.backup.backup_extensions,
            &cfg.backup.skip_extensions,
        ) {
            let key = object_key(&root, &path, &cfg.backup.key_prefix)?;
            let size = fs_util::file_size(&path)?;
            candidates.push(Candidate { key, path, size });
        }
    }
    candidates.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(candidates)
}

/// The cheap idempotency check: upload iff the object is missing or its size differs.
pub fn needs_upload(remote_size: Option<u64>, local_size: u64) -> bool {
    remote_size != Some(local_size)
}

/// HEAD every candidate (concurrently) and partition into "needs upload" vs "already
/// present". `progress`, if given, is ticked once per completed HEAD. The returned
/// `to_upload` preserves the candidates' (sorted) order regardless of completion order.
pub async fn plan<S: RemoteStore>(
    store: &S,
    candidates: &[Candidate],
    progress: Option<&ProgressBar>,
) -> Result<Plan, B2Error> {
    let mut decisions = vec![false; candidates.len()];

    let mut checks =
        futures::stream::iter(candidates.iter().enumerate().map(|(i, c)| async move {
            let remote = store.head(&c.key).await;
            (i, c.size, remote)
        }))
        .buffer_unordered(HEAD_CONCURRENCY);

    while let Some((i, size, remote)) = checks.next().await {
        decisions[i] = needs_upload(remote?, size);
        if let Some(pb) = progress {
            pb.inc(1);
        }
    }

    let mut to_upload = Vec::new();
    let mut skipped = 0usize;
    for (i, candidate) in candidates.iter().enumerate() {
        if decisions[i] {
            to_upload.push(candidate.clone());
        } else {
            skipped += 1;
        }
    }
    Ok(Plan { to_upload, skipped })
}

/// A missing or unreadable *local* file won't fix itself, so those fail fast. Everything
/// else (remote/network/throttling) is worth retrying.
fn is_transient(err: &B2Error) -> bool {
    !matches!(err, B2Error::Io(_))
}

/// Upload one file, retrying transient failures with exponential backoff per `policy`.
async fn put_with_retry<S: RemoteStore>(
    store: &S,
    item: &Candidate,
    policy: RetryPolicy,
) -> Result<(), B2Error> {
    let mut attempt = 0u32;
    loop {
        match store.put(&item.key, &item.path).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if attempt >= policy.attempts || !is_transient(&err) {
                    return Err(err);
                }
                let delay = policy.delay_for(attempt);
                tracing::warn!(
                    "upload of {} failed ({err}); retrying in {:?} (attempt {}/{})",
                    item.key,
                    delay,
                    attempt + 1,
                    policy.attempts
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// Upload `items`, keeping up to `concurrency` uploads in flight (a single stream rarely
/// saturates a fast uplink). Each completed file is recorded in `progress`. Returns on the
/// first non-retryable error. Completion order is nondeterministic, but every file is
/// independently HEAD-gated, so a resumed run is still correct.
pub async fn upload<S: RemoteStore>(
    store: &S,
    items: &[Candidate],
    progress: &Progress,
    concurrency: usize,
    policy: RetryPolicy,
) -> Result<(), B2Error> {
    let mut uploads = futures::stream::iter(items.iter().map(|item| async move {
        put_with_retry(store, item, policy)
            .await
            .map(|()| item.size)
    }))
    .buffer_unordered(concurrency.max(1));

    while let Some(result) = uploads.next().await {
        progress.record(result?);
    }
    Ok(())
}

/// Group pending uploads by lowercased extension, sorted largest-bytes-first, so the user
/// can see e.g. how much of the sync is video vs RAW before committing.
fn ext_breakdown(items: &[Candidate]) -> Vec<(String, usize, u64)> {
    let mut map: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for c in items {
        let ext = c
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_else(|| "(no ext)".to_string());
        let entry = map.entry(ext).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += c.size;
    }
    let mut rows: Vec<(String, usize, u64)> =
        map.into_iter().map(|(k, (n, b))| (k, n, b)).collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    rows
}

fn print_upload_summary(plan: &Plan) {
    println!(
        "{} file(s) ({}) to upload:",
        plan.to_upload.len(),
        fs_util::human_bytes(plan.pending_bytes())
    );
    for (ext, count, bytes) in ext_breakdown(&plan.to_upload) {
        println!("  {ext}: {} ({count} file(s))", fs_util::human_bytes(bytes));
    }
}

#[derive(Debug, Serialize)]
struct BackupReport {
    dry_run: bool,
    cancelled: bool,
    uploaded: usize,
    skipped: usize,
    bytes_uploaded: u64,
    pending: usize,
    pending_bytes: u64,
    keys: Vec<String>,
}

/// CLI entry point: resolve credentials, build the client, plan, then (for a real run)
/// confirm and upload.
pub fn run(cfg: &Config, dry_run: bool, json: bool, yes: bool) -> anyhow::Result<()> {
    let candidates = gather_candidates(cfg)?;

    let credentials = config::resolve_credentials()?.ok_or_else(|| {
        anyhow!(
            "no B2 credentials found. Run `photoflow login` or set {ENV_KEY_ID} and {ENV_APP_KEY}."
        )
    })?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building async runtime")?;

    let store = B2Store::new(&cfg.backup, &credentials)?;

    // Planning HEADs the whole tree; show a spinner so it's clearly working, not hung.
    let spinner = (!json).then(|| {
        let pb = ProgressBar::new(candidates.len() as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} Checking B2 for existing files… {pos}/{len}",
            )
            .expect("static spinner template is valid"),
        );
        pb.enable_steady_tick(TICK_INTERVAL);
        pb
    });
    let plan = runtime.block_on(plan(&store, &candidates, spinner.as_ref()))?;
    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    if dry_run {
        return report_dry_run(&plan, json);
    }

    if plan.to_upload.is_empty() {
        if json {
            return print_json(&BackupReport {
                dry_run: false,
                cancelled: false,
                uploaded: 0,
                skipped: plan.skipped,
                bytes_uploaded: 0,
                pending: 0,
                pending_bytes: 0,
                keys: Vec::new(),
            });
        }
        println!(
            "All {} backup-eligible file(s) are already in B2. Nothing to upload.",
            plan.skipped
        );
        return Ok(());
    }

    if !json {
        print_upload_summary(&plan);
    }

    // Confirm before uploading on an interactive terminal (unless --yes). Uploading is not
    // destructive, but we still surface what's about to leave the machine.
    if !yes && super::stdin_is_interactive() && !super::confirm("Proceed? [y/N] ")? {
        println!("Aborted; nothing uploaded.");
        return Ok(());
    }

    let progress = Progress::with_bar({
        let pb = ProgressBar::new(plan.pending_bytes());
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} ({percent}%) {bytes_per_sec} ETA {eta}  (Ctrl-C to cancel)",
            )
            .expect("static progress template is valid"),
        );
        pb.enable_steady_tick(TICK_INTERVAL);
        pb
    });

    // Race the upload against Ctrl-C. On signal, the upload future is dropped, which aborts
    // the in-flight request; files already uploaded remain, so a re-run resumes from there.
    let cancelled = runtime.block_on(async {
        tokio::select! {
            res = upload(
                &store,
                &plan.to_upload,
                &progress,
                cfg.backup.concurrency,
                RetryPolicy::standard(),
            ) => res.map(|()| false),
            _ = tokio::signal::ctrl_c() => Ok::<bool, B2Error>(true),
        }
    })?;
    progress.finish();

    let uploaded = progress.files_done();
    let bytes = progress.bytes_done();

    if cancelled {
        if json {
            return print_json(&BackupReport {
                dry_run: false,
                cancelled: true,
                uploaded,
                skipped: plan.skipped,
                bytes_uploaded: bytes,
                pending: plan.to_upload.len() - uploaded,
                pending_bytes: plan.pending_bytes().saturating_sub(bytes),
                keys: Vec::new(),
            });
        }
        println!(
            "Cancelled. Uploaded {uploaded} of {} file(s) ({}) before stopping; re-run `photoflow backup` to resume.",
            plan.to_upload.len(),
            fs_util::human_bytes(bytes)
        );
        return Ok(());
    }

    if json {
        print_json(&BackupReport {
            dry_run: false,
            cancelled: false,
            uploaded,
            skipped: plan.skipped,
            bytes_uploaded: bytes,
            pending: 0,
            pending_bytes: 0,
            keys: plan.to_upload.iter().map(|c| c.key.clone()).collect(),
        })?;
    } else {
        println!(
            "Uploaded {uploaded} file(s) ({}); {} already present.",
            fs_util::human_bytes(bytes),
            plan.skipped
        );
    }

    Ok(())
}

fn report_dry_run(plan: &Plan, json: bool) -> anyhow::Result<()> {
    if json {
        return print_json(&BackupReport {
            dry_run: true,
            cancelled: false,
            uploaded: 0,
            skipped: plan.skipped,
            bytes_uploaded: 0,
            pending: plan.to_upload.len(),
            pending_bytes: plan.pending_bytes(),
            keys: plan.to_upload.iter().map(|c| c.key.clone()).collect(),
        });
    }

    if plan.to_upload.is_empty() {
        println!(
            "Up to date: all {} backup-eligible file(s) are already in B2.",
            plan.skipped
        );
        return Ok(());
    }

    for candidate in &plan.to_upload {
        println!(
            "  {}  ({})",
            candidate.key,
            fs_util::human_bytes(candidate.size)
        );
    }
    print_upload_summary(plan);
    println!(
        "{} already present. (dry run — nothing uploaded)",
        plan.skipped
    );
    Ok(())
}

fn print_json(report: &BackupReport) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}
