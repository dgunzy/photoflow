//! Folder naming and date helpers.
//!
//! Month directories are `MM-Month` (e.g. `06-June`) — the numeric prefix guarantees
//! chronological sort. Export leaf dirs are `YYYY-MM-DD-<slug>`.

use chrono::{Datelike, Local, NaiveDate};
use thiserror::Error;

/// English month names, indexed by `month - 1`. Defined here (rather than relying on
/// locale-dependent `chrono` formatting) so the on-disk names are stable and predictable.
pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Date format for the year directory, e.g. `2026`.
pub const YEAR_FORMAT_WIDTH: usize = 4;
/// `strftime` pattern for export leaf-dir dates, e.g. `2026-06-15`.
pub const EXPORT_DATE_FORMAT: &str = "%Y-%m-%d";
/// Human description of the `--date` format accepted by `month`.
pub const YEAR_MONTH_FORMAT: &str = "YYYY-MM";
/// Human description of the `--date` format accepted by `export`.
pub const FULL_DATE_FORMAT: &str = "YYYY-MM-DD";

#[derive(Debug, Error)]
pub enum NamingError {
    #[error("invalid month {0}: must be 1-12")]
    InvalidMonth(u32),
    #[error("invalid date {input:?}: expected {expected}")]
    InvalidDate {
        input: String,
        expected: &'static str,
    },
}

/// `2026` (zero-padded to at least four digits).
pub fn year_dir_name(year: i32) -> String {
    format!("{year:0width$}", width = YEAR_FORMAT_WIDTH)
}

/// `6` -> `"06-June"`. Errors if `month` is not in `1..=12`.
pub fn month_dir_name(month: u32) -> Result<String, NamingError> {
    if !(1..=12).contains(&month) {
        return Err(NamingError::InvalidMonth(month));
    }
    let name = MONTH_NAMES[(month - 1) as usize];
    Ok(format!("{month:02}-{name}"))
}

/// Lowercase, collapse runs of spaces/underscores/hyphens into single hyphens, strip
/// every other non-alphanumeric character, and trim leading/trailing hyphens.
///
/// `"Charlie Bean!"` -> `"charlie-bean"`.
pub fn export_slug(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_separator = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('-');
            }
            pending_separator = false;
            out.push(ch.to_ascii_lowercase());
        } else if ch == ' ' || ch == '_' || ch == '-' {
            // Defer emitting a hyphen until we know a real char follows, so runs collapse
            // and trailing separators never make it into the output.
            pending_separator = true;
        }
        // Any other character (punctuation, symbols) is stripped entirely.
    }
    out
}

/// `2026-06-15` + `charlie-bean` -> `"2026-06-15-charlie-bean"`.
pub fn export_leaf_name(date: NaiveDate, slug: &str) -> String {
    format!("{}-{slug}", date.format(EXPORT_DATE_FORMAT))
}

/// Parse `"YYYY-MM"` into `(year, month)`.
pub fn parse_year_month(s: &str) -> Result<(i32, u32), NamingError> {
    let err = || NamingError::InvalidDate {
        input: s.to_string(),
        expected: YEAR_MONTH_FORMAT,
    };
    let (year, month) = s.split_once('-').ok_or_else(err)?;
    let year: i32 = year.parse().map_err(|_| err())?;
    let month: u32 = month.parse().map_err(|_| err())?;
    if !(1..=12).contains(&month) {
        return Err(NamingError::InvalidMonth(month));
    }
    Ok((year, month))
}

/// Parse `"YYYY-MM-DD"` into a [`NaiveDate`].
pub fn parse_date(s: &str) -> Result<NaiveDate, NamingError> {
    NaiveDate::parse_from_str(s, EXPORT_DATE_FORMAT).map_err(|_| NamingError::InvalidDate {
        input: s.to_string(),
        expected: FULL_DATE_FORMAT,
    })
}

/// Current local `(year, month)`.
pub fn current_year_month() -> (i32, u32) {
    let today = Local::now().date_naive();
    (today.year(), today.month())
}

/// Current local date.
pub fn current_date() -> NaiveDate {
    Local::now().date_naive()
}
