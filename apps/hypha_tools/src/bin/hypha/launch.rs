//! `hypha launch` -- runs a set of nodes in one tmux session.

#![forbid(unsafe_code)]

use hypha_tools::launch_cli::{LaunchCommand, help_text, parse_launch_arguments};
use hypha_tools::launch_session::session_name;
use hypha_tools::launch_supervisor::supervise_unit;
use hypha_tools::launch_tmux::{DOWN_WAIT_LIMIT, Tmux, session_attach, session_down, session_up};
use hypha_tools::launch_units::parse_units_file;
use std::env;
use std::fs;
use std::process::ExitCode;

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let args: Vec<String> = args.into_iter().skip(1).collect();
    let command = match parse_launch_arguments(&args) {
        Ok(Some(command)) => command,
        Ok(None) => {
            print!("{}", help_text());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprint!("{message}");
            return ExitCode::FAILURE;
        }
    };
    match run(command) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("hypha launch: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: LaunchCommand) -> Result<ExitCode, String> {
    let tmux = Tmux::default_server();
    match command {
        LaunchCommand::Up { units_file, session, attach } => {
            let units_file = fs::canonicalize(&units_file)
                .map_err(|error| format!("cannot read '{}': {error}", units_file.display()))?;
            let parsed = parse_units_file(&units_file)?;
            let session = session_name(&units_file, session.as_deref())?;
            let launcher = env::current_exe()
                .map_err(|error| format!("cannot find the hypha executable: {error}"))?;
            session_up(&tmux, &parsed, &session, &launcher, &units_file)?;
            println!("[hypha launch] session '{session}' is up with {} units", parsed.units.len());
            if attach {
                session_attach(&tmux, &session)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        LaunchCommand::Attach { units_file, session } => {
            let session = session_name(&units_file, session.as_deref())?;
            session_attach(&tmux, &session)?;
            Ok(ExitCode::SUCCESS)
        }
        LaunchCommand::Down { units_file, session } => {
            let session = session_name(&units_file, session.as_deref())?;
            session_down(&tmux, &session, DOWN_WAIT_LIMIT)?;
            println!("[hypha launch] session '{session}' is down");
            Ok(ExitCode::SUCCESS)
        }
        LaunchCommand::Unit { units_file, unit_name } => {
            let parsed = parse_units_file(&units_file)?;
            let config =
                parsed.units.iter().find(|unit| unit.name == unit_name).ok_or_else(|| {
                    format!("no unit named '{unit_name}' in '{}'", units_file.display())
                })?;
            supervise_unit(config).map(ExitCode::from)
        }
    }
}
