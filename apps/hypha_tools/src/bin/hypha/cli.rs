//! Start-up steps the tools share: the command line, the bus node and the stop signals.

use clap::Parser;
use clap::error::ErrorKind;
use hypha_node::sdk::BusNode;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Parses the whole command line `args`. `Err` carries the exit code once help or the
/// parse error is printed: success for `--help`, failure for an invalid command line.
pub(crate) fn parse_cli<C: Parser>(args: &[String]) -> Result<C, ExitCode> {
    match C::try_parse_from(args) {
        Ok(cli) => Ok(cli),
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            print!("{error}");
            Err(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprint!("{error}");
            Err(ExitCode::FAILURE)
        }
    }
}

/// Connects the node `node_name`; a failure is printed after `prefix`, and `Err` is the
/// failure exit code.
pub(crate) fn connect(prefix: &str, node_name: &str) -> Result<BusNode, ExitCode> {
    BusNode::new(node_name).map_err(|error| {
        eprintln!("{prefix}{error}");
        ExitCode::FAILURE
    })
}

/// A flag that SIGINT and SIGTERM set; a failure to register is printed after `prefix`,
/// and `Err` is the failure exit code.
pub(crate) fn stop_on_signals(prefix: &str) -> Result<Arc<AtomicBool>, ExitCode> {
    let stop_signal = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(error) = signal_hook::flag::register(signal, Arc::clone(&stop_signal)) {
            eprintln!("{prefix}cannot register stop handler: {error}");
            return Err(ExitCode::FAILURE);
        }
    }
    Ok(stop_signal)
}
