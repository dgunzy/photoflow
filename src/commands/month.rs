//! `photoflow month [--date YYYY-MM]` — create `<photos_root>/<YYYY>/<MM-Month>/`.
//! Idempotent: if the directory already exists, succeed and print its path.

use anyhow::Context;

use crate::config::Config;
use crate::naming;

pub fn run(cfg: &Config, date: Option<String>) -> anyhow::Result<()> {
    let (year, month) = match date {
        Some(s) => naming::parse_year_month(&s)?,
        None => naming::current_year_month(),
    };

    let dir = cfg
        .photos_root()
        .join(naming::year_dir_name(year))
        .join(naming::month_dir_name(month)?);

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating month directory {}", dir.display()))?;

    // Print the absolute path so the user can `cd` to it for the USB import.
    println!("{}", dir.display());
    Ok(())
}
