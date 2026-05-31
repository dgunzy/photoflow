//! Configuration: load/write/default the TOML config, expand `~` in paths, and resolve
//! B2 credentials from the environment or OS keyring.
//!
//! Credentials are *never* written to the config file or logged — only the keyring (via
//! `photoflow login`) or the `PHOTOFLOW_B2_*` environment variables hold them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Application name, used for the platform config dir and as the keyring service name.
pub const APP_NAME: &str = "photoflow";
/// Config file name inside the platform config dir.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// Keyring service under which credentials are stored.
pub const KEYRING_SERVICE: &str = "photoflow";
/// Keyring account holding the B2 key id.
pub const KEYRING_ACCOUNT_KEY_ID: &str = "b2_key_id";
/// Keyring account holding the B2 application key.
pub const KEYRING_ACCOUNT_APP_KEY: &str = "b2_app_key";
/// Environment variable for the B2 key id (takes precedence over the keyring).
pub const ENV_KEY_ID: &str = "PHOTOFLOW_B2_KEY_ID";
/// Environment variable for the B2 application key.
pub const ENV_APP_KEY: &str = "PHOTOFLOW_B2_APP_KEY";

/// The default config written by `photoflow init`. It is the single source of truth for
/// defaults: [`Config::defaults`] parses this string, and a unit test asserts it parses,
/// so the documented comments and the in-code defaults can never drift apart.
pub const DEFAULT_CONFIG_TOML: &str = r#"[paths]
photos_root  = "~/Pictures/Photos"
exports_root = "~/Pictures/Exports"

[clean]
# Files with these extensions (case-insensitive) are throwaway and get trashed by `clean`.
discard_extensions = ["jpg", "jpeg"]
# When trashing FILE.jpg, also trash a sibling FILE.jpg.xmp if present.
discard_sidecars   = true
# Directory names (case-insensitive) that `clean` never descends into. These hold finished
# JPEGs (e.g. exports kept under photos_root), which must never be trashed.
exclude_dirs = ["Exports"]

[backup]
endpoint = "https://s3.ca-east-006.backblazeb2.com"
region   = "ca-east-006"
bucket   = "photo-backup-dgunzy"
# Only files with these extensions (case-insensitive) are uploaded. RAWs, videos, and the
# Lightroom XMP sidecars that carry the edits. Everything else stays local.
backup_extensions = ["nef", "dng", "mov", "mp4", "heic", "xmp"]
# Extensions (case-insensitive) to NEVER upload, even if present in backup_extensions.
# Leave empty to upload everything eligible. Example: skip videos with ["mov", "mp4"].
skip_extensions = []
# How many files to upload in parallel. A single stream rarely saturates a fast uplink;
# 4-8 is a good range. Lower it to be gentler on the connection.
concurrency = 6
# Object keys are paths relative to photos_root, optionally under this prefix.
key_prefix = ""

[export]
# Recognized export categories. Free-form; these are just the ones scaffolded with nice defaults.
categories = ["instagram", "portfolio", "personal"]
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub paths: Paths,
    pub clean: CleanConfig,
    pub backup: BackupConfig,
    pub export: ExportConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    pub photos_root: String,
    pub exports_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanConfig {
    pub discard_extensions: Vec<String>,
    pub discard_sidecars: bool,
    /// Directory names `clean` never descends into (case-insensitive). `serde(default)`
    /// keeps configs written before this field existed loadable, defaulting to `Exports`.
    #[serde(default = "default_exclude_dirs")]
    pub exclude_dirs: Vec<String>,
}

fn default_exclude_dirs() -> Vec<String> {
    vec!["Exports".to_string()]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupConfig {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub backup_extensions: Vec<String>,
    /// Extensions never uploaded, even if listed in `backup_extensions`. `serde(default)`
    /// keeps pre-existing configs (without this key) loadable; default is empty (skip none).
    #[serde(default)]
    pub skip_extensions: Vec<String>,
    /// Parallel upload count. `serde(default)` keeps old configs loadable.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    pub key_prefix: String,
}

fn default_concurrency() -> usize {
    6
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportConfig {
    pub categories: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not determine platform config directory")]
    NoConfigDir,
    #[error("failed to read config at {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config at {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to write config at {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("keyring error: {0}")]
    Keyring(#[from] keyring::Error),
}

impl Config {
    /// The built-in defaults, parsed from [`DEFAULT_CONFIG_TOML`].
    pub fn defaults() -> Config {
        toml::from_str(DEFAULT_CONFIG_TOML)
            .expect("DEFAULT_CONFIG_TOML must always parse into Config")
    }

    /// Load and parse the config at `path`.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// `photos_root` with `~` expanded.
    pub fn photos_root(&self) -> PathBuf {
        expand_tilde(&self.paths.photos_root)
    }

    /// `exports_root` with `~` expanded.
    pub fn exports_root(&self) -> PathBuf {
        expand_tilde(&self.paths.exports_root)
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(path).into_owned())
}

/// Resolve the config path: the `--config` override if given, else the platform default
/// (`~/.config/photoflow/config.toml` on Linux, `~/Library/Application Support/photoflow/`
/// on macOS).
pub fn resolve_config_path(override_path: Option<PathBuf>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    let dirs = directories::ProjectDirs::from("", "", APP_NAME).ok_or(ConfigError::NoConfigDir)?;
    Ok(dirs.config_dir().join(CONFIG_FILE_NAME))
}

/// Write the default config to `path`, creating parent dirs. Callers are responsible for
/// the "don't overwrite an existing file" check (see `init`).
pub fn write_default_config(path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, DEFAULT_CONFIG_TOML).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// A resolved B2 credential pair (B2 `keyID` + `applicationKey`, used as S3
/// access-key-id / secret-access-key).
#[derive(Debug, Clone)]
pub struct Credentials {
    pub key_id: String,
    pub app_key: String,
}

/// Resolve credentials: environment variables first, then the OS keyring. Returns `None`
/// if neither source yields a complete pair.
pub fn resolve_credentials() -> Result<Option<Credentials>, ConfigError> {
    if let (Ok(key_id), Ok(app_key)) = (std::env::var(ENV_KEY_ID), std::env::var(ENV_APP_KEY)) {
        if !key_id.is_empty() && !app_key.is_empty() {
            return Ok(Some(Credentials { key_id, app_key }));
        }
    }
    match (
        keyring_get(KEYRING_ACCOUNT_KEY_ID)?,
        keyring_get(KEYRING_ACCOUNT_APP_KEY)?,
    ) {
        (Some(key_id), Some(app_key)) => Ok(Some(Credentials { key_id, app_key })),
        _ => Ok(None),
    }
}

/// Store credentials in the keyring, overwriting any prior values (idempotent).
pub fn store_credentials(credentials: &Credentials) -> Result<(), ConfigError> {
    keyring_entry(KEYRING_ACCOUNT_KEY_ID)?.set_password(&credentials.key_id)?;
    keyring_entry(KEYRING_ACCOUNT_APP_KEY)?.set_password(&credentials.app_key)?;
    Ok(())
}

fn keyring_entry(account: &str) -> Result<keyring::Entry, ConfigError> {
    Ok(keyring::Entry::new(KEYRING_SERVICE, account)?)
}

fn keyring_get(account: &str) -> Result<Option<String>, ConfigError> {
    match keyring_entry(account)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(other) => Err(ConfigError::Keyring(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_parses_and_has_expected_values() {
        let cfg = Config::defaults();
        assert_eq!(cfg.paths.photos_root, "~/Pictures/Photos");
        assert_eq!(cfg.clean.discard_extensions, vec!["jpg", "jpeg"]);
        assert!(cfg.clean.discard_sidecars);
        assert_eq!(cfg.clean.exclude_dirs, vec!["Exports"]);
        assert_eq!(cfg.backup.region, "ca-east-006");
        assert!(cfg.backup.backup_extensions.iter().any(|e| e == "nef"));
        assert_eq!(cfg.backup.key_prefix, "");
        assert!(
            cfg.backup.skip_extensions.is_empty(),
            "default skips nothing"
        );
        assert_eq!(cfg.backup.concurrency, 6);
    }

    #[test]
    fn config_without_exclude_dirs_still_loads_with_default() {
        // A config written before `exclude_dirs` existed must still parse.
        let legacy = r#"
[paths]
photos_root  = "~/Pictures/Photos"
exports_root = "~/Pictures/Exports"
[clean]
discard_extensions = ["jpg"]
discard_sidecars   = true
[backup]
endpoint = "https://example.com"
region   = "r"
bucket   = "b"
backup_extensions = ["nef"]
key_prefix = ""
[export]
categories = ["instagram"]
"#;
        let cfg: Config = toml::from_str(legacy).unwrap();
        assert_eq!(cfg.clean.exclude_dirs, vec!["Exports"]);
        // Backup keys added later also fall back to defaults.
        assert!(cfg.backup.skip_extensions.is_empty());
        assert_eq!(cfg.backup.concurrency, default_concurrency());
    }

    #[test]
    fn tilde_expands_to_absolute_path() {
        let cfg = Config::defaults();
        assert!(cfg.photos_root().is_absolute());
    }
}
