//! Command line of `hypha launch`.

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use std::path::PathBuf;

/// `hypha launch` runs a set of nodes in one tmux session.
#[derive(Debug, Parser)]
#[command(
    name = "hypha launch",
    after_help = "The session name is the file name without its extension, unless --session \
                  gives one. tmux 3.0 or newer is required."
)]
struct LaunchCli {
    #[command(subcommand)]
    command: LaunchCommand,
}

/// One `hypha launch` invocation.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum LaunchCommand {
    /// Start each unit in its own window of a detached tmux session.
    Up {
        /// Units file.
        units_file: PathBuf,
        /// Session name, instead of the file name without its extension.
        #[arg(long)]
        session: Option<String>,
        /// Attach this terminal after the session is built.
        #[arg(long)]
        attach: bool,
    },
    /// Attach this terminal to the session (detach again with Ctrl-b d).
    Attach {
        /// Units file that names the session.
        units_file: PathBuf,
        /// Session name, instead of the file name without its extension.
        #[arg(long)]
        session: Option<String>,
    },
    /// Stop every unit, then remove the session.
    Down {
        /// Units file that names the session.
        units_file: PathBuf,
        /// Session name, instead of the file name without its extension.
        #[arg(long)]
        session: Option<String>,
    },
    /// Supervise one unit. `up` runs this in each window, not an operator.
    Unit {
        /// Units file.
        units_file: PathBuf,
        /// Name of the unit to supervise.
        unit_name: String,
    },
}

/// Parses the arguments after the program name. `Ok(None)` means `--help` was asked for.
///
/// # Errors
///
/// Returns the text `clap` prints for a missing or unknown subcommand, an unknown option,
/// an option without its value, a repeated option, or a missing or extra argument.
pub fn parse_launch_arguments(args: &[String]) -> Result<Option<LaunchCommand>, String> {
    let words = std::iter::once("hypha launch").chain(args.iter().map(String::as_str));
    match LaunchCli::try_parse_from(words) {
        Ok(cli) => Ok(Some(cli.command)),
        Err(error) if error.kind() == ErrorKind::DisplayHelp => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// The help text of `hypha launch`: the summary, then the help of each subcommand, so that
/// every option of `up`, `attach`, `down` and `unit` is listed.
#[must_use]
pub fn help_text() -> String {
    let mut command = LaunchCli::command();
    let mut text = command.render_help().to_string();
    let names: Vec<String> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .filter(|name| name != "help")
        .collect();
    for name in names {
        if let Some(sub) = command.find_subcommand_mut(&name) {
            sub.set_bin_name(format!("hypha launch {name}"));
            text.push('\n');
            text.push_str(&sub.render_help().to_string());
        }
    }
    text
}
