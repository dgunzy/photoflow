//! `photoflow status [--remote]` — a read-only library overview of both trees (Photos and
//! Exports), with per-group counts of RAW / video / JPEG / xmp / other. No network unless
//! `--remote` is passed, in which case it reports how much is not yet backed up using the
//! same HEAD/size logic as `backup --dry-run`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context};
use indicatif::{ProgressBar, ProgressStyle};

use crate::b2::B2Store;
use crate::commands::{backup, clean};
use crate::config::{self, Config, ENV_APP_KEY, ENV_KEY_ID};
use crate::fs_util;

/// Extensions treated as video for the RAW/video split in the summary. The RAW/video
/// distinction is presentation-only (config has no such split), so this small heuristic
/// lives here rather than in config.
const VIDEO_EXTENSIONS: [&str; 2] = ["mov", "mp4"];
/// Lightroom sidecar extension, counted separately in the summary.
const XMP_EXTENSION: &str = "xmp";

#[derive(Debug, Default, Clone)]
struct Counts {
    raw: usize,
    video: usize,
    jpeg: usize,
    xmp: usize,
    other: usize,
    bytes: u64,
}

impl Counts {
    fn add(&mut self, other: &Counts) {
        self.raw += other.raw;
        self.video += other.video;
        self.jpeg += other.jpeg;
        self.xmp += other.xmp;
        self.other += other.other;
        self.bytes += other.bytes;
    }

    fn files(&self) -> usize {
        self.raw + self.video + self.jpeg + self.xmp + self.other
    }

    fn line(&self) -> String {
        format!(
            "{} raw, {} video, {} jpeg, {} xmp, {} other  ({})",
            self.raw,
            self.video,
            self.jpeg,
            self.xmp,
            self.other,
            fs_util::human_bytes(self.bytes)
        )
    }
}

fn classify(path: &Path, cfg: &Config, counts: &mut Counts, size: u64) {
    counts.bytes += size;
    if fs_util::has_extension(path, &cfg.clean.discard_extensions) {
        counts.jpeg += 1;
    } else if fs_util::has_extension(path, &[XMP_EXTENSION.to_string()]) {
        counts.xmp += 1;
    } else if fs_util::has_extension(path, &VIDEO_EXTENSIONS.map(String::from)) {
        counts.video += 1;
    } else if fs_util::has_extension(path, &cfg.backup.backup_extensions) {
        counts.raw += 1;
    } else {
        counts.other += 1;
    }
}

/// Group a photos path into `"YYYY/MM-Month"` from its first two components; files directly
/// under the root are grouped as `"(root)"`.
fn month_group(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [year, month, ..] => format!("{year}/{month}"),
        _ => "(root)".to_string(),
    }
}

/// Group an exports path by its top-level category dir; files directly under the root are
/// grouped as `"(root)"`.
fn category_group(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    match rel.components().next() {
        Some(first) if rel.components().count() > 1 => {
            first.as_os_str().to_string_lossy().into_owned()
        }
        _ => "(root)".to_string(),
    }
}

/// Walk `root` (recursively, skipping `exclude` dirs), classify every file, and bucket the
/// counts by `group_fn`. Returns the per-group map plus the grand total.
fn summarize(
    root: &Path,
    cfg: &Config,
    exclude: &[String],
    group_fn: impl Fn(&Path, &Path) -> String,
) -> (BTreeMap<String, Counts>, Counts) {
    let mut groups: BTreeMap<String, Counts> = BTreeMap::new();
    let mut total = Counts::default();
    for path in fs_util::walk_files_excluding(root, exclude) {
        let size = fs_util::file_size(&path).unwrap_or(0);
        let mut counts = Counts::default();
        classify(&path, cfg, &mut counts, size);
        total.add(&counts);
        groups
            .entry(group_fn(root, &path))
            .or_default()
            .add(&counts);
    }
    (groups, total)
}

fn print_section(title: &str, root: &Path, groups: &BTreeMap<String, Counts>, total: &Counts) {
    println!("{title}: {}", root.display());
    if groups.is_empty() {
        println!("  (empty)");
    }
    for (group, counts) in groups {
        println!("  {group}: {}", counts.line());
    }
    println!(
        "  Total: {} file(s); {}",
        total.files(),
        fs_util::human_bytes(total.bytes)
    );
}

pub fn run(cfg: &Config, remote: bool) -> anyhow::Result<()> {
    // Photos overview excludes the export dirs nested under photos_root so they aren't
    // double-counted with the Exports section below.
    let photos_root = cfg.photos_root();
    let (photo_groups, photo_total) =
        summarize(&photos_root, cfg, &cfg.clean.exclude_dirs, month_group);
    print_section("Photos", &photos_root, &photo_groups, &photo_total);

    println!();

    let exports_root = cfg.exports_root();
    let (export_groups, export_total) = summarize(&exports_root, cfg, &[], category_group);
    print_section("Exports", &exports_root, &export_groups, &export_total);

    // Count what `clean` would actually remove (same exclusions), which can be fewer than
    // the total JPEGs present — e.g. finished exports under photos_root are protected.
    let would_remove = clean::plan_clean(
        &photos_root,
        &cfg.clean.discard_extensions,
        cfg.clean.discard_sidecars,
        &cfg.clean.exclude_dirs,
    )
    .into_iter()
    .filter(|v| fs_util::has_extension(&v.path, &cfg.clean.discard_extensions))
    .count();
    println!("\nJPEGs `clean` would remove: {would_remove}");

    if remote {
        report_remote(cfg)?;
    }

    Ok(())
}

fn report_remote(cfg: &Config) -> anyhow::Result<()> {
    let candidates = backup::gather_candidates(cfg)?;
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

    let spinner = ProgressBar::new(candidates.len() as u64);
    spinner.set_style(
        ProgressStyle::with_template("{spinner:.green} Checking B2… {pos}/{len}")
            .expect("static spinner template is valid"),
    );
    spinner.enable_steady_tick(Duration::from_millis(120));
    let plan = runtime.block_on(backup::plan(&store, &candidates, Some(&spinner)))?;
    spinner.finish_and_clear();

    println!(
        "Remote: {} of {} backup-eligible file(s) not yet in B2 ({} to upload).",
        plan.to_upload.len(),
        candidates.len(),
        fs_util::human_bytes(plan.pending_bytes())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::Path;

    fn count(name: &str) -> Counts {
        let cfg = Config::defaults();
        let mut c = Counts::default();
        classify(Path::new(name), &cfg, &mut c, 10);
        c
    }

    #[test]
    fn classify_buckets_by_extension() {
        assert_eq!(count("a.jpg").jpeg, 1);
        assert_eq!(count("a.JPEG").jpeg, 1);
        assert_eq!(count("a.NEF").raw, 1);
        assert_eq!(count("clip.MOV").video, 1);
        assert_eq!(count("a.NEF.xmp").xmp, 1);
        assert_eq!(count("notes.txt").other, 1);
        // The reported bug: a JPEG must never be counted as raw.
        assert_eq!(count("a.jpg").raw, 0, "jpg must not be classified raw");
    }
}
