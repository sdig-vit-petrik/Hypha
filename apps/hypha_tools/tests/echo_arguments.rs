//! Tests for the `hypha topic echo` command line.
#![allow(clippy::unwrap_used)]

use clap::Parser;
use hypha_tools::echo_args::EchoArgs;

/// The words after `echo`, as `hypha topic` flattens them into its subcommand.
#[derive(Debug, Parser)]
#[command(name = "hypha_topic echo")]
struct EchoCli {
    #[command(flatten)]
    echo: EchoArgs,
}

fn parse(words: &[&str]) -> Result<EchoArgs, String> {
    let argv = std::iter::once("hypha_topic echo").chain(words.iter().copied());
    EchoCli::try_parse_from(argv).map(|cli| cli.echo).map_err(|error| error.to_string())
}

#[test]
fn topic_alone_uses_defaults() {
    let args = parse(&["/state"]).unwrap();
    assert_eq!(args, EchoArgs { topic: "/state".into(), max_count: 0, plot: false });
}

#[test]
fn plot_flag_may_come_before_or_after_the_topic() {
    assert!(parse(&["-p", "/state"]).unwrap().plot);
    assert!(parse(&["/state", "-p"]).unwrap().plot);
}

#[test]
fn count_takes_a_number() {
    assert_eq!(parse(&["/state", "--count", "5"]).unwrap().max_count, 5);
}

#[test]
fn count_without_a_number_is_refused() {
    assert!(parse(&["/state", "--count"]).is_err());
    assert!(parse(&["/state", "--count", "many"]).is_err());
}

#[test]
fn unknown_flag_is_refused() {
    assert!(parse(&["/state", "--colour"]).unwrap_err().contains("--colour"));
}

#[test]
fn missing_topic_is_refused() {
    assert!(parse(&["-p"]).is_err());
    assert!(parse(&[]).is_err());
}

#[test]
fn second_topic_is_refused() {
    assert!(parse(&["/a", "/b"]).is_err());
}
