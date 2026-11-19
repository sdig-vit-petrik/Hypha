//! `hypha param` -- inspect a TOML parameter file as a node loads it. The file is
//! edited by hand.

use crate::cli::parse_cli;
use clap::{Parser, Subcommand};
use hypha_node::sdk::parameters::{ParamValue, Parameters};
use std::path::Path;
use std::process::ExitCode;

/// Inspect a TOML parameter file as a node loads it.
#[derive(Debug, Parser)]
#[command(
    name = "hypha param",
    after_help = "Edit the file by hand. A running node reads its own copy at start-up."
)]
struct Cli {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Every scalar key=value pair, sorted.
    Dump {
        /// TOML parameter file.
        file: String,
    },
    /// One scalar value, or exit 1 if absent.
    Get {
        /// TOML parameter file.
        file: String,
        /// Dotted key, such as controller.baz.
        key: String,
    },
    /// Load the file as a node does: includes, ${NAME} and duplicate keys.
    Validate {
        /// TOML parameter file.
        file: String,
    },
}

/// The parameters of `path`, loaded as a node loads its file: includes, `${NAME}`
/// references, relative paths and the refusal of a key two files set.
fn load(path: &str) -> Result<Parameters, String> {
    Parameters::from_toml_file(Path::new(path)).map_err(|error| error.to_string())
}

/// A value as `dump` and `get` print it: text without quotes.
fn value_text(value: &ParamValue) -> String {
    match value {
        ParamValue::Text(text) => text.clone(),
        other => other.to_string(),
    }
}

fn run_dump(path: &str) -> ExitCode {
    let parameters = match load(path) {
        Ok(parameters) => parameters,
        Err(error) => {
            eprintln!("hypha param: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (key, value) in parameters.iter() {
        pairs.push((key.to_owned(), value_text(value)));
    }
    pairs.sort();
    for (key, value) in pairs {
        println!("{key}={value}");
    }
    ExitCode::SUCCESS
}

fn run_get(path: &str, key: &str) -> ExitCode {
    let parameters = match load(path) {
        Ok(parameters) => parameters,
        Err(error) => {
            eprintln!("hypha param: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(value) = parameters.value(key) else {
        eprintln!("hypha param: no scalar key '{key}' in '{path}'");
        return ExitCode::FAILURE;
    };
    println!("{}", value_text(value));
    ExitCode::SUCCESS
}

fn run_validate(path: &str) -> ExitCode {
    match load(path) {
        Ok(_) => {
            println!("{path}: valid");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("hypha param: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli: Cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    match cli.verb {
        Verb::Dump { file } => run_dump(&file),
        Verb::Get { file, key } => run_get(&file, &key),
        Verb::Validate { file } => run_validate(&file),
    }
}
