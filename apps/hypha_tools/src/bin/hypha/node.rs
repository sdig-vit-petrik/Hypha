//! `hypha node` -- process introspection for the hypha bus: list and info.

use crate::cli::{connect, parse_cli};
use clap::{Parser, Subcommand};
use hypha_node::sdk::NodeInfo;
use hypha_node::sdk::{BusNode, domain};
use std::process::ExitCode;

/// Process introspection for the hypha bus: list and info.
#[derive(Debug, Parser)]
#[command(
    name = "hypha node",
    after_help = "There is no per-node record of what a node publishes or subscribes to -- a raw \
                  bus sample carries no publisher identity, only its topic and type."
)]
struct Cli {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Every process the bus's node registry currently knows about, alive or dead.
    List,
    /// Detail for the nodes of one name (name, executable, pid); the processes of
    /// one CLI tool share its name.
    Info {
        /// Node name.
        name: String,
    },
}

fn display_name(node: &NodeInfo) -> &str {
    if node.name.is_empty() { "<unnamed>" } else { &node.name }
}

fn print_one_line(node: &NodeInfo) {
    let state = if node.is_alive { "alive" } else { "dead" };
    let exe = if node.executable.is_empty() { "?" } else { &node.executable };
    println!("{state:<6} {:<28} pid={:<8} exe={exe}", display_name(node), node.pid);
}

fn run_list(bus: &BusNode) -> ExitCode {
    let nodes = match bus.list_nodes() {
        Ok(nodes) => nodes,
        Err(cause) => {
            eprintln!("hypha node: {cause}");
            return ExitCode::FAILURE;
        }
    };
    if nodes.is_empty() {
        println!(
            "(no nodes -- either nothing is running yet, or this backend has no node registry to query)"
        );
        return ExitCode::SUCCESS;
    }
    for node in &nodes {
        print_one_line(node);
    }
    ExitCode::SUCCESS
}

fn run_info(bus: &BusNode, name: &str) -> ExitCode {
    let nodes = match bus.list_nodes() {
        Ok(nodes) => nodes,
        Err(cause) => {
            eprintln!("hypha node: {cause}");
            return ExitCode::FAILURE;
        }
    };
    // Every process of a CLI tool holds the tool's name, so one name can list several nodes.
    let mut matching: Vec<&NodeInfo> = Vec::new();
    for node in &nodes {
        if node.name == name {
            matching.push(node);
        }
    }
    if matching.is_empty() {
        eprintln!("hypha node: no node named '{name}' (currently alive or recently dead)");
        return ExitCode::FAILURE;
    }

    let selected_domain = match domain() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("hypha node: {error}");
            return ExitCode::FAILURE;
        }
    };
    for node in matching {
        println!(
            "Node [{}]
  state:      {}
  pid:        {}
  executable: {}
  domain:     {}
",
            display_name(node),
            if node.is_alive { "alive" } else { "dead" },
            node.pid,
            if node.executable.is_empty() { "?" } else { &node.executable },
            if selected_domain.is_empty() { "<none>" } else { &selected_domain },
        );
    }

    println!("Publications and subscriptions: not available from the node registry");

    ExitCode::SUCCESS
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli: Cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let bus = match connect("hypha node: ", "_hyphacli_node") {
        Ok(bus) => bus,
        Err(code) => return code,
    };

    match cli.verb {
        Verb::List => run_list(&bus),
        Verb::Info { name } => run_info(&bus, &name),
    }
}
