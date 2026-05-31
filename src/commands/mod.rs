//! One module per subcommand. Each exposes a `run(...)` entry point invoked by the clap
//! dispatch in `main.rs`. The dangerous operations (`clean`, `backup`) additionally
//! expose trait-abstracted core logic (planning, a `Trasher`/`RemoteStore` seam) so they
//! can be tested without trashing real files or hitting the network.

pub mod backup;
pub mod clean;
pub mod export;
pub mod init;
pub mod login;
pub mod month;
pub mod status;

use std::io::{self, IsTerminal, Write};

/// Prompt for a yes/no confirmation on an interactive terminal. Returns `true` if the user
/// answers yes. Used to guard operations that permanently affect user data.
pub(crate) fn confirm(prompt: &str) -> anyhow::Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// True if stdin is an interactive terminal (so prompting makes sense).
pub(crate) fn stdin_is_interactive() -> bool {
    io::stdin().is_terminal()
}
