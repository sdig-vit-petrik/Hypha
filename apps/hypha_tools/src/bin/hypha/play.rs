//! `hypha play` -- replays an MCAP recording onto the hypha bus.

use crate::cli::{connect, stop_on_signals};
use clap::Parser;
use clap::error::ErrorKind;
use hypha_tools::play_session::{PlaySessionConfig, SyncConfig, run_play_session};
use hypha_tools::player::Player;
use hypha_tools::replay_sync::{
    DEFAULT_CHUNK_NANOS, DEFAULT_SETTLE, DEFAULT_TIMEOUT, Pace, parse_chunk_nanos, parse_pace,
    parse_seconds,
};
use hypha_tools::utc_time::{UtcRange, parse_utc_nanos};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

/// A one-time wait, paid only on exit, for a subscriber (e.g. `hypha foxglove`) that
/// connects right at the end of a short or fast-forwarded playback.
const SHUTDOWN_SETTLE_TIME: Duration = Duration::from_millis(350);

/// Command line of `hypha play`. Flags that only `--clock` uses are `None` when they
/// were not given, so a replay without `--clock` can refuse them.
#[derive(Debug, Parser)]
#[command(
    name = "hypha play",
    about = "Replays an MCAP recording onto the hypha bus.",
    after_help = "Supports interactive keyboard shortcuts:\n  <Space> Pause/Resume   <s> Step one \
                  chunk   <+>/<-> Rate   <r> Restart   <q> Quit"
)]
struct Cli {
    /// MCAP recording to replay.
    #[arg(
        short = 'i',
        long = "input",
        value_name = "file",
        default_value = "",
        hide_default_value = true
    )]
    input: String,
    /// Playback speed multiplier (default: 1.0), or max for no pacing.
    #[arg(short = 'r', long = "rate", value_name = "factor|max", value_parser = parse_pace)]
    pace: Option<Pace>,
    /// Loop playback until interrupted.
    #[arg(short = 'l', long = "loop")]
    loop_playback: bool,
    /// Publish /clock and replay in chunks that every node has finished.
    #[arg(long = "clock")]
    publish_clock: bool,
    /// Whitelist of topic names to replay.
    #[arg(long = "topics", value_name = "t", num_args = 1..)]
    topics: Vec<String>,
    /// Inclusive UTC start, e.g. 2026-10-08T12:30:00Z.
    #[arg(long = "start-utc", value_name = "time", value_parser = parse_start_utc)]
    start_utc_nanos: Option<u64>,
    /// Exclusive UTC end.
    #[arg(long = "end-utc", value_name = "time", value_parser = parse_end_utc)]
    end_utc_nanos: Option<u64>,
    /// With --clock: nodes that must take part; the start fails after 30 s if one is missing.
    #[arg(long = "nodes", value_name = "name", num_args = 1..)]
    nodes: Vec<String>,
    /// With --clock: start once no new node has reported for this long (default: 2 s).
    #[arg(long = "settle", value_name = "s", value_parser = parse_settle)]
    settle: Option<Duration>,
    /// With --clock: data time of one chunk (default: 100 ms).
    #[arg(long = "chunk", value_name = "ms", value_parser = parse_chunk_nanos)]
    chunk_nanos: Option<u64>,
    /// With --clock: stop when a node reports nothing for this long (default: 5 s).
    #[arg(long = "timeout", value_name = "s", value_parser = parse_timeout)]
    timeout: Option<Duration>,
}

impl Cli {
    /// The sync rules of a `--clock` replay, with the defaults for what was not given.
    fn sync(&self, lockstep: bool) -> SyncConfig {
        SyncConfig {
            chunk_nanos: self.chunk_nanos.unwrap_or(DEFAULT_CHUNK_NANOS),
            nodes: self.nodes.clone(),
            settle: self.settle.unwrap_or(DEFAULT_SETTLE),
            timeout: self.timeout.unwrap_or(DEFAULT_TIMEOUT),
            lockstep,
        }
    }
}

/// The variable that turns the replay lockstep off for a whole launch; every node reads
/// it too, so the player and the nodes agree.
const LOCKSTEP_VARIABLE: &str = "HYPHA_REPLAY_LOCKSTEP";

/// Whether `text`, the value of [`LOCKSTEP_VARIABLE`], keeps the lockstep: `1` or
/// `true` keep it, `0` or `false` turn it off, anything else is refused, as at node
/// creation.
fn parse_lockstep(text: &str) -> Result<bool, String> {
    match text {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!("{LOCKSTEP_VARIABLE} must be 1, true, 0 or false, not '{text}'")),
    }
}

/// Whether the environment keeps the replay lockstep; an unset variable keeps it.
fn lockstep_requested() -> Result<bool, String> {
    match std::env::var(LOCKSTEP_VARIABLE) {
        Ok(text) => parse_lockstep(&text),
        Err(std::env::VarError::NotPresent) => Ok(true),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{LOCKSTEP_VARIABLE} is not readable text"))
        }
    }
}

/// Reads a UTC timestamp for the flag `arg`.
///
/// # Errors
/// Returns a message that names the flag when the timestamp is not valid.
fn parse_utc(arg: &str, text: &str) -> Result<u64, String> {
    parse_utc_nanos(text).map_err(|cause| format!("{arg}: {cause}"))
}

fn parse_start_utc(text: &str) -> Result<u64, String> {
    parse_utc("--start-utc", text)
}

fn parse_end_utc(text: &str) -> Result<u64, String> {
    parse_utc("--end-utc", text)
}

/// Reads a duration in seconds that must be above zero.
fn non_zero_seconds(flag: &str, text: &str) -> Result<Duration, String> {
    let time = parse_seconds(flag, text)?;
    if time.is_zero() {
        return Err(format!("{flag} needs a time above zero"));
    }
    Ok(time)
}

fn parse_settle(text: &str) -> Result<Duration, String> {
    non_zero_seconds("--settle", text)
}

fn parse_timeout(text: &str) -> Result<Duration, String> {
    non_zero_seconds("--timeout", text)
}

/// Parses `args` (excluding argv\[0]) into `Cli`, for a replay in the lockstep when
/// `lockstep`. `Ok(None)` means `--help` was requested and already printed; `Err` means
/// the command line is invalid and a message was already printed -- the caller refuses
/// to start.
fn parse_args(args: &[String], lockstep: bool) -> Result<Option<Cli>, ()> {
    let words = std::iter::once("hypha play").chain(args.iter().map(String::as_str));
    let cli = match Cli::try_parse_from(words) {
        Ok(cli) => cli,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            print!("{error}");
            return Ok(None);
        }
        Err(error) => {
            eprint!("{error}");
            return Err(());
        }
    };

    let mut ok = true;
    if !cli.publish_clock {
        let sync_flags = [
            (!cli.nodes.is_empty(), "--nodes"),
            (cli.settle.is_some(), "--settle"),
            (cli.chunk_nanos.is_some(), "--chunk"),
            (cli.timeout.is_some(), "--timeout"),
        ];
        for (given, flag) in sync_flags {
            if given {
                eprintln!("hypha play: {flag} takes effect only with --clock");
                ok = false;
            }
        }
    }

    if cli.publish_clock && !lockstep {
        // Open loop: nothing paces a replay at max, and no participant is waited for.
        if cli.pace == Some(Pace::Max) {
            eprintln!(
                "hypha play: --rate max needs the replay lockstep; with {LOCKSTEP_VARIABLE}=0 nothing would pace the replay"
            );
            ok = false;
        }
        let lockstep_flags = [
            (!cli.nodes.is_empty(), "--nodes"),
            (cli.settle.is_some(), "--settle"),
            (cli.timeout.is_some(), "--timeout"),
        ];
        for (given, flag) in lockstep_flags {
            if given {
                eprintln!(
                    "hypha play: {flag} takes effect only in the replay lockstep, not with {LOCKSTEP_VARIABLE}=0"
                );
                ok = false;
            }
        }
    }

    if ok { Ok(Some(cli)) } else { Err(()) }
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let lockstep = match lockstep_requested() {
        Ok(lockstep) => lockstep,
        Err(cause) => {
            eprintln!("hypha play: {cause}, refusing to start");
            return ExitCode::FAILURE;
        }
    };
    let cli = match parse_args(args.get(1..).unwrap_or(&[]), lockstep) {
        Ok(Some(cli)) => cli,
        Ok(None) => return ExitCode::SUCCESS,
        Err(()) => {
            eprintln!("hypha play: invalid command line, refusing to start");
            return ExitCode::FAILURE;
        }
    };

    if cli.input.is_empty() {
        eprintln!("hypha play: --input <file> is required");
        return ExitCode::FAILURE;
    }
    let pace = cli.pace.unwrap_or(Pace::Factor(1.0));

    let utc_range = match UtcRange::new(cli.start_utc_nanos, cli.end_utc_nanos) {
        Ok(range) => range,
        Err(cause) => {
            eprintln!("hypha play: {cause}");
            return ExitCode::FAILURE;
        }
    };
    let stop_signal = match stop_on_signals("hypha play: ") {
        Ok(flag) => flag,
        Err(code) => return code,
    };
    let bus = match connect("hypha play: ", "hypha_play") {
        Ok(bus) => bus,
        Err(code) => return code,
    };

    let player = Player::new(&cli.input);
    if !player.is_open() {
        eprintln!("hypha play: cannot open '{}': {}", cli.input, player.open_error());
        return ExitCode::FAILURE;
    }

    println!("Playing back '{}' (rate: {pace}, loop: {})...", cli.input, cli.loop_playback);
    for topic in &cli.topics {
        println!("hypha play: narrowed to '{topic}'");
    }
    if cli.publish_clock && !lockstep {
        println!(
            "hypha play: open loop ({LOCKSTEP_VARIABLE}=0): /clock and the data follow --rate, and no node is waited for"
        );
    }

    let config = PlaySessionConfig {
        pace,
        loop_playback: cli.loop_playback,
        publish_clock: cli.publish_clock,
        topics: cli.topics.clone(),
        utc_range,
        sync: cli.sync(lockstep),
    };

    let result = run_play_session(&bus, &player, config, &stop_signal);

    for (topic, messages) in &result.counters {
        println!("hypha play: '{topic}' published {messages} messages");
    }
    if let Some(rate) = &result.rate {
        println!("hypha play: {rate}");
    }
    println!("hypha play: done, {} message(s) published from '{}'", result.published, cli.input);

    if let Some(error) = result.error {
        eprintln!("hypha play: {error}");
        return ExitCode::FAILURE;
    }
    thread::sleep(SHUTDOWN_SETTLE_TIME);

    ExitCode::SUCCESS
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::{Cli, parse_args, parse_lockstep};
    use clap::Parser as _;
    use clap::error::ErrorKind;
    use hypha_tools::replay_sync::{DEFAULT_SETTLE, DEFAULT_TIMEOUT, Pace};
    use std::time::Duration;

    fn parse(args: &[&str]) -> Result<Cli, ()> {
        parse_in(args, true)
    }

    fn parse_in(args: &[&str], lockstep: bool) -> Result<Cli, ()> {
        let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        parse_args(&owned, lockstep).map(|cli| cli.expect("not --help"))
    }

    #[test]
    fn the_lockstep_variable_reads_one_true_zero_or_false_and_refuses_the_rest() {
        assert_eq!(parse_lockstep("1"), Ok(true));
        assert_eq!(parse_lockstep("true"), Ok(true));
        assert_eq!(parse_lockstep("0"), Ok(false));
        assert_eq!(parse_lockstep("false"), Ok(false));
        assert!(parse_lockstep("no").is_err());
    }

    #[test]
    fn an_open_loop_replay_refuses_rate_max_and_the_flags_of_the_lockstep() {
        for flags in [
            vec!["--rate", "max"],
            vec!["--nodes", "posmaster"],
            vec!["--settle", "1"],
            vec!["--timeout", "2"],
        ] {
            let mut args = vec!["--input", "a.mcap", "--clock"];
            args.extend(flags.iter().copied());
            assert!(parse_in(&args, false).is_err(), "{flags:?} without the lockstep");
            assert!(parse_in(&args, true).is_ok(), "{flags:?} in the lockstep");
        }
        let open =
            parse_in(&["--input", "a.mcap", "--clock", "--rate", "4", "--chunk", "20"], false)
                .expect("an open-loop replay at a rate");
        assert!(!open.sync(false).lockstep);
        // Without --clock there is no lockstep to turn off: max stays allowed.
        assert!(parse_in(&["--input", "a.mcap", "--rate", "max"], false).is_ok());
    }

    #[test]
    fn rate_accepts_a_factor_or_max() {
        assert_eq!(
            parse(&["--input", "a.mcap", "--rate", "4"]).expect("parses").pace,
            Some(Pace::Factor(4.0))
        );
        assert_eq!(
            parse(&["--input", "a.mcap", "--rate", "max"]).expect("parses").pace,
            Some(Pace::Max)
        );
        assert!(parse(&["--input", "a.mcap", "--rate", "0"]).is_err());
        assert!(parse(&["--input", "a.mcap", "--rate"]).is_err());
    }

    #[test]
    fn nodes_read_names_up_to_the_next_flag() {
        let cli =
            parse(&["--input", "a.mcap", "--clock", "--nodes", "posmaster", "recorder", "--loop"])
                .expect("parses");
        assert_eq!(cli.nodes, vec!["posmaster".to_string(), "recorder".to_string()]);
        assert!(cli.loop_playback, "the flag after the names is read");
    }

    #[test]
    fn nodes_without_a_name_is_refused() {
        assert!(parse(&["--input", "a.mcap", "--clock", "--nodes"]).is_err());
        assert!(parse(&["--input", "a.mcap", "--clock", "--nodes", "--loop"]).is_err());
    }

    #[test]
    fn the_sync_flags_need_clock() {
        for flags in [
            vec!["--nodes", "posmaster"],
            vec!["--settle", "1"],
            vec!["--chunk", "50"],
            vec!["--timeout", "2"],
        ] {
            let mut args = vec!["--input", "a.mcap"];
            args.extend(flags.iter().copied());
            assert!(parse(&args).is_err(), "{flags:?} without --clock");
        }
    }

    #[test]
    fn the_sync_flags_read_their_units() {
        let cli = parse(&[
            "--input",
            "a.mcap",
            "--clock",
            "--settle",
            "0.5",
            "--chunk",
            "250",
            "--timeout",
            "3",
        ])
        .expect("parses");
        let sync = cli.sync(true);
        assert_eq!(sync.settle, Duration::from_millis(500));
        assert_eq!(sync.chunk_nanos, 250_000_000);
        assert_eq!(sync.timeout, Duration::from_secs(3));
        assert_eq!(sync.nodes.len(), 0);
    }

    #[test]
    fn the_defaults_are_the_settle_chunk_and_timeout_of_the_spec() {
        let sync = parse(&["--input", "a.mcap", "--clock"]).expect("parses").sync(true);
        assert_eq!(sync.settle, DEFAULT_SETTLE);
        assert_eq!(sync.chunk_nanos, 100_000_000);
        assert_eq!(sync.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn a_zero_settle_is_refused() {
        assert!(parse(&["--input", "a.mcap", "--clock", "--settle", "0"]).is_err());
        assert!(parse(&["--input", "a.mcap", "--clock", "--settle", "0.1"]).is_ok());
    }

    #[test]
    fn a_zero_chunk_and_a_zero_timeout_are_refused() {
        assert!(parse(&["--input", "a.mcap", "--clock", "--chunk", "0"]).is_err());
        assert!(parse(&["--input", "a.mcap", "--clock", "--timeout", "0"]).is_err());
    }

    #[test]
    fn play_refuses_lossy_and_max_lag_as_unknown_arguments() {
        for flag in ["--lossy", "--max-lag"] {
            let words = ["hypha_play", "--input", "a.mcap", "--clock", "--rate", "4", flag, "50"];
            let error = Cli::try_parse_from(words).expect_err("the flag is not a known argument");
            assert_eq!(error.kind(), ErrorKind::UnknownArgument);
            assert!(error.to_string().contains(flag), "{error}");
            // The path main takes: a refused command line, which exits 1.
            assert!(parse(&["--input", "a.mcap", "--clock", flag, "50"]).is_err(), "{flag}");
        }
    }

    #[test]
    fn the_removed_flags_are_unrecognised() {
        assert!(parse(&["--input", "a.mcap", "--wait-for", "2"]).is_err());
        assert!(parse(&["--input", "a.mcap", "--queue-depth", "64"]).is_err());
    }
}
