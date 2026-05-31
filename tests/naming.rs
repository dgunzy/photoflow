//! Unit tests for folder naming, slugs, and date parsing.

use chrono::NaiveDate;
use photoflow::naming;

#[test]
fn month_dir_names_have_numeric_prefix() {
    assert_eq!(naming::month_dir_name(6).unwrap(), "06-June");
    assert_eq!(naming::month_dir_name(11).unwrap(), "11-November");
    assert_eq!(naming::month_dir_name(1).unwrap(), "01-January");
    assert_eq!(naming::month_dir_name(12).unwrap(), "12-December");
}

#[test]
fn month_dir_name_rejects_out_of_range() {
    assert!(naming::month_dir_name(0).is_err());
    assert!(naming::month_dir_name(13).is_err());
}

#[test]
fn year_dir_name_is_zero_padded() {
    assert_eq!(naming::year_dir_name(2026), "2026");
    assert_eq!(naming::year_dir_name(99), "0099");
}

#[test]
fn slug_lowercases_collapses_and_strips() {
    assert_eq!(naming::export_slug("Charlie Bean!"), "charlie-bean");
    assert_eq!(naming::export_slug("  Hello__World  "), "hello-world");
    assert_eq!(naming::export_slug("a---b"), "a-b");
    assert_eq!(
        naming::export_slug("Trip 2026: Iceland"),
        "trip-2026-iceland"
    );
    assert_eq!(naming::export_slug("!!!"), "");
}

#[test]
fn export_leaf_name_is_date_then_slug() {
    let date = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    assert_eq!(
        naming::export_leaf_name(date, "charlie-bean"),
        "2026-06-15-charlie-bean"
    );
}

#[test]
fn parse_year_month_round_trips() {
    assert_eq!(naming::parse_year_month("2026-06").unwrap(), (2026, 6));
    assert!(naming::parse_year_month("2026-13").is_err());
    assert!(naming::parse_year_month("nope").is_err());
}

#[test]
fn parse_date_validates_format() {
    assert_eq!(
        naming::parse_date("2026-06-15").unwrap(),
        NaiveDate::from_ymd_opt(2026, 6, 15).unwrap()
    );
    assert!(naming::parse_date("2026-6-15").is_err() || naming::parse_date("2026-06-32").is_err());
    assert!(naming::parse_date("not-a-date").is_err());
}
