//! `photoflow` binary: argument parsing and dispatch only. All behavior lives in the
//! `photoflow` library crate.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing::Level;

use photoflow::commands;
use photoflow::config::{self, Config};

#[derive(Parser)]
#[command(
    name = "photoflow",
    version,
    about = "Manage a personal photography library: dated folders, JPEG cleanup, B2 backup, export scaffolding."
)]
struct Cli {
    /// Use this config file instead of the platform default.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Enable verbose (debug) logging.
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write the default config (if absent) and ensure the photos/exports roots exist.
    Init,

    /// Prompt for B2 credentials (hidden) and store them in the OS keyring.
    Login,

    /// Create `<photos_root>/<YYYY>/<MM-Month>/` for the import.
    Month {
        /// Month to create, as YYYY-MM. Defaults to the current month.
        #[arg(long, value_name = "YYYY-MM")]
        date: Option<String>,
    },

    /// Trash throwaway JPEGs (and their .xmp sidecars). Dry-run unless --apply.
    ///
    /// NOTE: every JPEG under PATH is treated as throwaway. Deletion goes to the OS trash,
    /// so it is recoverable.
    Clean {
        /// Directory to clean. Defaults to photos_root.
        path: Option<PathBuf>,
        /// Actually move files to the OS trash (otherwise dry-run).
        #[arg(long)]
        apply: bool,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Upload originals (RAW/video/xmp) to Backblaze B2. Fully idempotent.
    Backup {
        /// Show what would upload without uploading anything.
        #[arg(long)]
        dry_run: bool,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Skip the interactive confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },

    /// Create `<exports_root>/<category>/<YYYY-MM-DD>-<slug>/`.
    Export {
        /// Export category (e.g. instagram, portfolio, personal).
        category: String,
        /// Human name for the export; slugified for the directory.
        name: String,
        /// Date as YYYY-MM-DD. Defaults to today.
        #[arg(long, value_name = "YYYY-MM-DD")]
        date: Option<String>,
    },

    /// Read-only library overview. --remote also reports what is not yet in B2.
    Status {
        /// Check the remote for un-backed-up files (requires credentials).
        #[arg(long)]
        remote: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(verbose: bool) {
    let level = if verbose { Level::DEBUG } else { Level::WARN };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .without_time()
        .init();
}

fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let config_path = config::resolve_config_path(cli.config)?;

    match cli.command {
        Command::Init => commands::init::run(&config_path),
        Command::Login => commands::login::run(),
        Command::Month { date } => commands::month::run(&load_config(&config_path)?, date),
        Command::Clean { path, apply, json } => {
            commands::clean::run(&load_config(&config_path)?, path, apply, json)
        }
        Command::Backup { dry_run, json, yes } => {
            commands::backup::run(&load_config(&config_path)?, dry_run, json, yes)
        }
        Command::Export {
            category,
            name,
            date,
        } => commands::export::run(&load_config(&config_path)?, category, name, date),
        Command::Status { remote } => commands::status::run(&load_config(&config_path)?, remote),
    }
}

fn load_config(path: &std::path::Path) -> anyhow::Result<Config> {
    Config::load(path).with_context(|| {
        format!(
            "loading config at {} (run `photoflow init` to create it)",
            path.display()
        )
    })
}
