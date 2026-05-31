//! `photoflow clean [PATH] [--apply] [--json]` — trash throwaway files (by
//! `clean.discard_extensions`, case-insensitive) and, optionally, their `.xmp` sidecars.
//!
//! Dry-run by default: it only moves files to the OS trash when `--apply` is passed. The
//! actual trashing is behind the [`Trasher`] trait so the planning/idempotency logic can
//! be tested without touching the real OS trash.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Serialize;

use crate::config::Config;
use crate::fs_util;

/// Sidecar extension appended after the full filename: Lightroom writes `FILE.jpg.xmp`.
pub const SIDECAR_EXTENSION: &str = "xmp";

/// Abstraction over "move these paths to the trash", so tests can substitute a fake.
pub trait Trasher {
    fn trash(&self, paths: &[PathBuf]) -> anyhow::Result<()>;
}

/// Production trasher: moves files to the OS trash (recoverable), never hard-unlinks.
pub struct OsTrasher;

impl Trasher for OsTrasher {
    fn trash(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        trash::delete_all(paths).context("moving files to the OS trash")?;
        Ok(())
    }
}

/// A file slated for trashing and its size (for the summary).
#[derive(Debug, Clone, Serialize)]
pub struct Victim {
    pub path: PathBuf,
    pub size: u64,
}

/// The `<file>.xmp` sidecar path for `file` (e.g. `DSC.jpg` -> `DSC.jpg.xmp`).
pub fn sidecar_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_os_string();
    s.push(".");
    s.push(SIDECAR_EXTENSION);
    PathBuf::from(s)
}

/// Collect every existing file under `root` whose extension is in `discard_extensions`,
/// plus each present `.xmp` sidecar when `discard_sidecars` is set. Directories named in
/// `exclude_dirs` (e.g. finished exports) are never descended into. Deterministically
/// sorted and de-duplicated.
pub fn plan_clean(
    root: &Path,
    discard_extensions: &[String],
    discard_sidecars: bool,
    exclude_dirs: &[String],
) -> Vec<Victim> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for file in fs_util::walk_files_excluding(root, exclude_dirs) {
        if fs_util::has_extension(&file, discard_extensions) {
            if discard_sidecars {
                let sidecar = sidecar_path(&file);
                if sidecar.is_file() {
                    paths.push(sidecar);
                }
            }
            paths.push(file);
        }
    }
    paths.sort();
    paths.dedup();

    paths
        .into_iter()
        .map(|path| {
            let size = fs_util::file_size(&path).unwrap_or(0);
            Victim { path, size }
        })
        .collect()
}

#[derive(Debug, Serialize)]
struct CleanReport<'a> {
    applied: bool,
    count: usize,
    total_bytes: u64,
    files: &'a [Victim],
}

pub fn run(cfg: &Config, path: Option<PathBuf>, apply: bool, json: bool) -> anyhow::Result<()> {
    let root = path.unwrap_or_else(|| cfg.photos_root());
    let victims = plan_clean(
        &root,
        &cfg.clean.discard_extensions,
        cfg.clean.discard_sidecars,
        &cfg.clean.exclude_dirs,
    );
    let total_bytes: u64 = victims.iter().map(|v| v.size).sum();

    if apply {
        let trasher = OsTrasher;
        let paths: Vec<PathBuf> = victims.iter().map(|v| v.path.clone()).collect();
        trasher.trash(&paths)?;
    }

    if json {
        let report = CleanReport {
            applied: apply,
            count: victims.len(),
            total_bytes,
            files: &victims,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if victims.is_empty() {
        println!("Nothing to clean under {}.", root.display());
        return Ok(());
    }

    let verb = if apply { "Trashed" } else { "Would trash" };
    for v in &victims {
        println!("  {}  ({})", v.path.display(), fs_util::human_bytes(v.size));
    }
    println!(
        "{verb} {} file(s), {} total.",
        victims.len(),
        fs_util::human_bytes(total_bytes)
    );
    if !apply {
        println!("This was a dry run. Re-run with --apply to move these files to the OS trash (recoverable).");
    }

    Ok(())
}
