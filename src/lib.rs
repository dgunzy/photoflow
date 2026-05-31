//! `photoflow` — a small CLI for managing a personal photography library.
//!
//! The binary (`main.rs`) is clap dispatch only. All behavior lives here so it can
//! be exercised by unit and integration tests without spawning the binary.
//!
//! Module map:
//! - [`config`] — load/write/default config, `~` expansion, credential resolution.
//! - [`naming`] — month dir names, export slugs, date parsing/formatting.
//! - [`fs_util`] — directory walking, case-insensitive extension matching, hidden-file
//!   filtering, and size helpers.
//! - [`b2`] — the [`b2::RemoteStore`] trait (trait-abstracted for testing) and its
//!   Backblaze-B2-over-S3 implementation.
//! - [`commands`] — one module per subcommand.

pub mod b2;
pub mod commands;
pub mod config;
pub mod fs_util;
pub mod naming;
