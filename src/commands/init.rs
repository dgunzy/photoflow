//! `photoflow init` — write the default config if absent, then ensure the photos and
//! exports roots exist. Idempotent: re-running never overwrites an existing config.

use std::path::Path;

use anyhow::Context;

use crate::config::{self, Config};

pub fn run(config_path: &Path) -> anyhow::Result<()> {
    if config_path.exists() {
        println!("Config already exists at {}", config_path.display());
    } else {
        config::write_default_config(config_path)?;
        println!("Wrote default config to {}", config_path.display());
    }

    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config at {}", config_path.display()))?;

    for (label, dir) in [
        ("photos_root", cfg.photos_root()),
        ("exports_root", cfg.exports_root()),
    ] {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating {label} at {}", dir.display()))?;
        println!("Ensured {label} at {}", dir.display());
    }

    Ok(())
}
