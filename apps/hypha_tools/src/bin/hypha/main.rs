//! `hypha` -- the command line tools of hypha in one executable.
//!
//! `hypha <tool> [arguments]` runs one tool. A start under an old name linked to this
//! executable (`hypha play`, `hypha topic`, ...) runs that tool.

#![forbid(unsafe_code)]

mod cli;
mod foxglove;
mod intercept;
mod launch;
mod node;
mod param;
mod play;
mod record;
mod top;
mod topic;

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

/// What a tool's entry point is: the whole command line in, the exit code out.
type ToolMain = fn(Vec<String>) -> ExitCode;

/// One tool: its subcommand, the old executable name linked to `hypha`, its entry
/// point and its line in `hypha --help`.
struct Tool {
    subcommand: &'static str,
    linked_name: &'static str,
    main: ToolMain,
    summary: &'static str,
}

/// Every tool, in the order `hypha --help` lists them.
const TOOLS: [Tool; 9] = [
    Tool {
        subcommand: "topic",
        linked_name: "hypha_topic",
        main: topic::main,
        summary: "list, echo, hz and bw of topics",
    },
    Tool {
        subcommand: "top",
        linked_name: "hypha_top",
        main: top::main,
        summary: "interactive bus monitor and node inspector",
    },
    Tool {
        subcommand: "node",
        linked_name: "hypha_node",
        main: node::main,
        summary: "list the nodes and show one",
    },
    Tool {
        subcommand: "record",
        linked_name: "hypha_record",
        main: record::main,
        summary: "record bus traffic into an MCAP file",
    },
    Tool {
        subcommand: "play",
        linked_name: "hypha_play",
        main: play::main,
        summary: "replay an MCAP recording onto the bus",
    },
    Tool {
        subcommand: "intercept",
        linked_name: "hypha_intercept",
        main: intercept::main,
        summary: "rewrite fields of topics in flight",
    },
    Tool {
        subcommand: "foxglove",
        linked_name: "hypha_foxglove_bridge",
        main: foxglove::main,
        summary: "WebSocket bridge to Foxglove Studio",
    },
    Tool {
        subcommand: "launch",
        linked_name: "hypha_launch",
        main: launch::main,
        summary: "run a set of nodes in one tmux session",
    },
    Tool {
        subcommand: "param",
        linked_name: "hypha_param",
        main: param::main,
        summary: "inspect a TOML parameter file",
    },
];

fn usage() -> String {
    let mut text = String::from("Usage: hypha <tool> [arguments]\n\nTools:\n");
    for tool in &TOOLS {
        let _ = writeln!(text, "  {:<10} {}", tool.subcommand, tool.summary);
    }
    text.push_str("\n`hypha <tool> --help` shows the arguments of one tool.\n");
    text
}

/// The tool a start under `program` (argv\[0]) names directly, through its link.
fn tool_of_linked_name(program: &str) -> Option<&'static Tool> {
    let file_name = Path::new(program).file_name().and_then(|name| name.to_str()).unwrap_or("");
    TOOLS.iter().find(|tool| tool.linked_name == file_name)
}

fn tool_of_subcommand(word: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.subcommand == word)
}

/// Runs `tool` with `arguments` (the words after the tool), showing it as
/// `hypha <tool>` in its usage and its messages.
fn run_tool(tool: &Tool, arguments: Vec<String>) -> ExitCode {
    let mut command_line = Vec::with_capacity(arguments.len().saturating_add(1));
    command_line.push(format!("hypha {}", tool.subcommand));
    command_line.extend(arguments);
    (tool.main)(command_line)
}

fn main() -> ExitCode {
    let mut words = std::env::args();
    let program = words.next().unwrap_or_default();
    if let Some(tool) = tool_of_linked_name(&program) {
        return run_tool(tool, words.collect());
    }

    let Some(first) = words.next() else {
        eprint!("{}", usage());
        return ExitCode::FAILURE;
    };
    let asks_for_help = first == "--help" || first == "-h" || first == "help";
    if asks_for_help {
        print!("{}", usage());
        return ExitCode::SUCCESS;
    }
    let Some(tool) = tool_of_subcommand(&first) else {
        eprint!("hypha: unknown tool '{first}'\n\n{}", usage());
        return ExitCode::FAILURE;
    };
    run_tool(tool, words.collect())
}
