//! Filesystem helpers shared across commands: directory walking, case-insensitive
//! extension matching, hidden/system-file filtering, and size formatting.

use std::path::{Path, PathBuf};

use walkdir::{DirEntry, WalkDir};

/// macOS Finder metadata file.
pub const DS_STORE: &str = ".DS_Store";
/// Windows thumbnail-cache file.
pub const THUMBS_DB: &str = "Thumbs.db";
/// Prefix of macOS AppleDouble resource-fork sidecars (e.g. `._DSC_0001.NEF`).
pub const APPLEDOUBLE_PREFIX: &str = "._";
/// Separator used when building object keys (forward slash, S3-style).
pub const KEY_SEPARATOR: &str = "/";

/// Binary size unit suffixes for [`human_bytes`].
const SIZE_UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
const SIZE_STEP: f64 = 1024.0;

/// True if `path`'s extension (the final `.ext`, case-insensitive) is in `extensions`.
///
/// `extensions` are bare extensions without the dot, e.g. `["nef", "mov"]`.
pub fn has_extension(path: &Path, extensions: &[String]) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => extensions.iter().any(|want| want.eq_ignore_ascii_case(ext)),
        None => false,
    }
}

/// True for dotfiles, AppleDouble files, `.DS_Store`, and `Thumbs.db`. These are never
/// counted, cleaned, or uploaded.
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with(APPLEDOUBLE_PREFIX)
        || name.eq_ignore_ascii_case(DS_STORE)
        || name.eq_ignore_ascii_case(THUMBS_DB)
}

fn entry_is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(is_hidden_name)
        .unwrap_or(false)
}

fn entry_is_excluded_dir(entry: &DirEntry, exclude_dir_names: &[String]) -> bool {
    entry.file_type().is_dir()
        && entry.file_name().to_str().is_some_and(|name| {
            exclude_dir_names
                .iter()
                .any(|excluded| excluded.eq_ignore_ascii_case(name))
        })
}

/// Recursively collect every regular file under `root`, skipping hidden/system files and
/// directories and never following symlinks (avoids loops and double-processing).
///
/// Unreadable entries are silently skipped rather than aborting the whole walk. The
/// returned list is unsorted; callers that need determinism should sort.
pub fn walk_files(root: &Path) -> Vec<PathBuf> {
    walk_files_excluding(root, &[])
}

/// Like [`walk_files`], but also prunes any directory whose name (case-insensitive) is in
/// `exclude_dir_names` — neither the directory nor its contents are visited. Used to keep
/// `clean` out of finished-export folders nested under `photos_root`.
pub fn walk_files_excluding(root: &Path, exclude_dir_names: &[String]) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        // `filter_entry` prunes hidden and excluded directories (we never descend into
        // them) as well as hidden files.
        .filter_entry(|e| !entry_is_hidden(e) && !entry_is_excluded_dir(e, exclude_dir_names))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(DirEntry::into_path)
        .collect()
}

/// Size of `path` in bytes.
pub fn file_size(path: &Path) -> std::io::Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}

/// Format a byte count as a human-readable string, e.g. `1536` -> `"1.50 KiB"`.
pub fn human_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= SIZE_STEP && unit < SIZE_UNITS.len() - 1 {
        value /= SIZE_STEP;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", SIZE_UNITS[0])
    } else {
        format!("{value:.2} {}", SIZE_UNITS[unit])
    }
}
