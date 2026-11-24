//! `hypha clean` -- remove the shared memory of the bus domains nobody uses.

use crate::cli::parse_cli;
use clap::Parser;
use hypha_node::sdk::clean::{CleanOutcome, CleanReport, DomainSelection, clean, domain_label};
use std::process::ExitCode;

/// Remove the shared memory of the bus domains nobody uses. Kills nothing.
#[derive(Debug, Parser)]
#[command(
    name = "hypha clean",
    after_help = "Without --domain it cleans the domain of HYPHA_DOMAIN, the domain \"none\" when \
                  unset.\n\nA domain is kept while one of its nodes lives, or, under --domain, while \
                  the process whose id follows the prefix lives (a test domain is t_<pid>_...).\n\n\
                  Exit codes: 0 removed, 1 failed, 2 nothing to remove, 3 kept a domain in use."
)]
struct Cli {
    /// Clean every domain whose name starts with this prefix.
    #[arg(long, value_name = "prefix")]
    domain: Option<String>,
}

/// The exit code of each outcome.
fn exit_code(outcome: CleanOutcome) -> ExitCode {
    match outcome {
        CleanOutcome::Removed => ExitCode::SUCCESS,
        CleanOutcome::Failed => ExitCode::FAILURE,
        CleanOutcome::NothingToDo => ExitCode::from(2),
        CleanOutcome::Kept => ExitCode::from(3),
    }
}

/// What the selection covers, for the last line.
fn scope(selection: &DomainSelection) -> String {
    match selection {
        DomainSelection::Prefix(prefix) => format!("the domains under '{prefix}'"),
        DomainSelection::Exact(domain) => domain_label(domain),
    }
}

fn print_report(selection: &DomainSelection, report: &CleanReport) {
    for removed in &report.removed {
        let noun = if removed.resources == 1 { "resource" } else { "resources" };
        println!(
            "hypha clean: removed {} ({} {noun})",
            domain_label(&removed.domain),
            removed.resources
        );
    }
    for kept in &report.kept {
        println!("hypha clean: kept {}: {}", domain_label(&kept.domain), kept.reason);
    }
    for failure in &report.failures {
        eprintln!("hypha clean: {failure}");
    }
    if report.outcome() == CleanOutcome::NothingToDo {
        println!("hypha clean: nothing to remove of {}", scope(selection));
    } else {
        println!(
            "hypha clean: of {}, removed {}, kept {}, failed {}",
            scope(selection),
            report.removed.len(),
            report.kept.len(),
            report.failures.len()
        );
    }
}

pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli = match parse_cli::<Cli>(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let selection = match cli.domain {
        Some(prefix) => DomainSelection::prefix(&prefix),
        None => hypha_node::sdk::domain().map(DomainSelection::Exact),
    };
    let selection = match selection {
        Ok(selection) => selection,
        Err(message) => {
            eprintln!("hypha clean: {message}");
            return ExitCode::FAILURE;
        }
    };
    let report = clean(&selection);
    print_report(&selection, &report);
    exit_code(report.outcome())
}
