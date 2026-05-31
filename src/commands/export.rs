//! `photoflow export <category> <name> [--date YYYY-MM-DD]` — create
//! `<exports_root>/<category>/<YYYY-MM-DD>-<slug>/`. Idempotent. Warns (does not fail) if
//! `category` is not one of the configured categories.

use anyhow::{bail, Context};
use tracing::warn;

use crate::config::Config;
use crate::naming;

pub fn run(
    cfg: &Config,
    category: String,
    name: String,
    date: Option<String>,
) -> anyhow::Result<()> {
    let date = match date {
        Some(s) => naming::parse_date(&s)?,
        None => naming::current_date(),
    };

    if !cfg.export.categories.iter().any(|c| c == &category) {
        warn!(
            "'{category}' is not a configured export category ({}); creating it anyway",
            cfg.export.categories.join(", ")
        );
        eprintln!("warning: '{category}' is not a configured export category; creating it anyway");
    }

    let slug = naming::export_slug(&name);
    if slug.is_empty() {
        bail!("name {name:?} produced an empty slug; choose a name with letters or digits");
    }

    let leaf = naming::export_leaf_name(date, &slug);
    let dir = cfg.exports_root().join(&category).join(&leaf);

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating export directory {}", dir.display()))?;

    println!("{}", dir.display());
    Ok(())
}
