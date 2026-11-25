//! `hypha load` -- runs a workload file and writes one report.

#![forbid(unsafe_code)]

use clap::Parser;
use clap::error::ErrorKind;
use hypha_tools::load::{EXIT_RUN_FAILED, coordinator, worker};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli::stop_on_signals;

/// Command line of `hypha load`.
#[derive(Debug, Parser)]
#[command(
    name = "hypha load",
    about = "Runs the publisher and subscriber processes of a workload file in a private \
             domain and writes one JSON report: latency, loss, missed periods and memory.",
    after_help = "Exit codes: 0 every limit passed, 1 a limit failed, 70 the run itself failed \
                  (an invalid command line or workload included)."
)]
struct Cli {
    /// The workload file (TOML).
    #[arg(value_name = "workload.toml", required_unless_present = "worker")]
    workload: Option<PathBuf>,
    /// The report to write (JSON).
    #[arg(long = "report", value_name = "file", required_unless_present = "worker")]
    report: Option<PathBuf>,
    /// The recording, when the workload turns the recorder on (default: the report's path
    /// with the extension .mcap).
    #[arg(long = "recording", value_name = "file")]
    recording: Option<PathBuf>,
    /// Runs as the worker process of this name; the coordinator starts it.
    #[arg(long = "worker", value_name = "process", hide = true, conflicts_with_all = ["workload", "report", "recording"])]
    worker: Option<String>,
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error)
            if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) =>
        {
            print!("{error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprint!("{error}");
            return ExitCode::from(EXIT_RUN_FAILED);
        }
    };
    if let Some(process) = cli.worker {
        return ExitCode::from(worker::run(&process));
    }
    let (Some(workload), Some(report)) = (cli.workload, cli.report) else {
        eprintln!("hypha load: a workload file and --report are required");
        return ExitCode::from(EXIT_RUN_FAILED);
    };
    let Ok(stop) = stop_on_signals("hypha load: ") else {
        return ExitCode::from(EXIT_RUN_FAILED);
    };
    let paths = coordinator::Paths { workload, report, recording: cli.recording };
    ExitCode::from(coordinator::run(&paths, &stop))
}
