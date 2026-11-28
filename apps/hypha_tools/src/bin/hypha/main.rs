//! `hypha` -- the command line tools of hypha in one executable.
//!
//! `hypha <tool> [arguments]` runs one tool, and so does `hy <tool> [arguments]`: `hy`
//! is a link to this executable. A start under the link `hy<tool>` (`hyplay`,
//! `hytopic`, ...) runs that tool. Usage and messages name the tool `hypha <tool>`.

#![forbid(unsafe_code)]

mod clean;
mod cli;
mod convert;
mod foxglove;
mod intercept;
mod launch;
mod load;
mod node;
mod param;
mod play;
mod record;
mod service;
mod top;
mod topic;

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

/// What a tool's entry point is: the whole command line in, the exit code out.
type ToolMain = fn(Vec<String>) -> ExitCode;

/// One tool: its subcommand, its entry point and its line in `hypha --help`. Its
/// linked name is `hy` and the subcommand.
struct Tool {
    subcommand: &'static str,
    main: ToolMain,
    summary: &'static str,
}

/// Every tool, in the order `hypha --help` lists them.
const TOOLS: [Tool; 13] = [
    Tool {
        subcommand: "topic",
        main: topic::main,
        summary: "list, echo, hz and bw of topics; the pool of a policy",
    },
    Tool {
        subcommand: "top",
        main: top::main,
        summary: "interactive bus monitor and node inspector",
    },
    Tool { subcommand: "node", main: node::main, summary: "list the nodes and show one" },
    Tool {
        subcommand: "service",
        main: service::main,
        summary: "list the services, show one, and call one",
    },
    Tool {
        subcommand: "record",
        main: record::main,
        summary: "record bus traffic into an MCAP file",
    },
    Tool { subcommand: "play", main: play::main, summary: "replay an MCAP recording onto the bus" },
    Tool {
        subcommand: "convert",
        main: convert::main,
        summary: "turn a ROS 2 bag into an MCAP recording",
    },
    Tool {
        subcommand: "intercept",
        main: intercept::main,
        summary: "rewrite fields of topics in flight",
    },
    Tool {
        subcommand: "foxglove",
        main: foxglove::main,
        summary: "WebSocket bridge to Foxglove Studio",
    },
    Tool {
        subcommand: "launch",
        main: launch::main,
        summary: "run a set of nodes in one tmux session",
    },
    Tool { subcommand: "param", main: param::main, summary: "inspect a TOML parameter file" },
    Tool {
        subcommand: "clean",
        main: clean::main,
        summary: "remove the shared memory of bus domains nobody uses",
    },
    Tool {
        subcommand: "load",
        main: load::main,
        summary: "run a workload and report latency, loss and memory",
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

/// The tool a start under `program` (argv\[0]) names directly, through its link
/// `hy<tool>`. `hy` and `hypha` name none.
fn tool_of_linked_name(program: &str) -> Option<&'static Tool> {
    let file_name = Path::new(program).file_name().and_then(|name| name.to_str())?;
    tool_of_subcommand(file_name.strip_prefix("hy")?)
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
