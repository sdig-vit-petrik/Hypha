//! `hypha foxglove` -- live WebSocket bridge to Foxglove Studio.

use crate::cli::{connect, parse_cli};
use clap::Parser;
use hypha_tools::foxglove::{BridgeConfig, FOXGLOVE_SUBPROTOCOL, FoxgloveBridge, ServerOptions};
use hypha_tools::stream::StreamSelector;
use hypha_tools::stream_follower::StreamFollower;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

/// Command line of `hypha foxglove`.
#[derive(Debug, Parser)]
#[command(
    name = "hypha foxglove",
    about = "Bridges live hypha topics to Foxglove Studio over WebSocket."
)]
struct Cli {
    /// WebSocket listen port.
    #[arg(
        short = 'p',
        long,
        value_name = "port",
        default_value_t = 8765,
        allow_hyphen_values = true
    )]
    port: u16,
    /// Listen address.
    #[arg(
        short = 'a',
        long,
        value_name = "addr",
        default_value = "0.0.0.0",
        allow_hyphen_values = true
    )]
    address: String,
    /// Drop a sample that waited longer for its client.
    #[arg(long, value_name = "ms", default_value_t = 500, value_parser = parse_latency_ms, allow_hyphen_values = true)]
    max_latency: u64,
    /// Maximum frame rate per topic (e.g. 5.0 for cellular).
    #[arg(long, value_name = "Hz", default_value_t = 0.0, allow_hyphen_values = true)]
    max_rate: f64,
    /// Topic patterns to serve (default: every topic).
    #[arg(long, num_args = 0.., value_name = "pattern")]
    topics: Vec<String>,
}

/// Reads `--max-latency` in milliseconds, which must be above zero.
fn parse_latency_ms(text: &str) -> Result<u64, String> {
    text.parse::<u64>()
        .ok()
        .filter(|millis| *millis > 0)
        .ok_or_else(|| format!("bad --max-latency value '{text}'"))
}

/// Pending samples one followed topic may buffer: zero, the depth of the topic's `QoS`
/// policy depth (8, the `default` policy), which a larger depth would exceed.
const SUBSCRIPTION_DEPTH: usize = 0;
/// Samples served per pass, so one busy topic cannot hold the loop open.
const DRAIN_BUDGET_PER_PASS: usize = 512;

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli: Cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let bus = match connect("hypha foxglove: ", "hypha_foxglove_bridge") {
        Ok(bus) => bus,
        Err(code) => return code,
    };

    let server_options = ServerOptions {
        address: cli.address,
        port: cli.port,
        max_frame_age: Duration::from_millis(cli.max_latency),
    };
    let address = server_options.address.clone();
    let config = BridgeConfig { topics: cli.topics, max_rate_hz: cli.max_rate, server_options };
    let bridge = match FoxgloveBridge::start(config) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "hypha foxglove: failed to bind and start WebSocket server on {address}:{}: {e}",
                cli.port
            );
            return ExitCode::FAILURE;
        }
    };

    println!("===============================================================");
    println!(" hypha_foxglove_bridge is listening on ws://{address}:{}", bridge.local_port());
    println!(" Subprotocol: {FOXGLOVE_SUBPROTOCOL}");
    println!(" Open Foxglove Studio -> Open connection -> Foxglove WebSocket");
    println!(" URL: ws://localhost:{}", bridge.local_port());
    println!("===============================================================\n");

    // A pattern subscription resolves its streams once, so the follower subscribes to each
    // stream as the graph shows it.
    let mut follower = match StreamFollower::new(
        &bus,
        bridge.subscription_patterns(),
        StreamSelector::default(),
        SUBSCRIPTION_DEPTH,
    ) {
        Ok(follower) => follower,
        Err(error) => {
            eprintln!("hypha foxglove: cannot follow the bus: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = follower.refresh(&bus) {
        eprintln!("hypha foxglove: cannot list topics: {error}");
    }

    let mut last_health_log = Instant::now();
    while bus.ok() {
        if let Err(error) = follower.refresh_if_changed(&bus) {
            eprintln!("hypha foxglove: cannot list topics: {error}");
        }
        let mut drained = 0_usize;
        while drained < DRAIN_BUDGET_PER_PASS {
            let next = follower.next_sample().unwrap_or_else(|error| {
                eprintln!("hypha foxglove: {error}");
                None
            });
            let Some(sample) = next else { break };
            drained = drained.saturating_add(1);
            if let (Some(identity), Some(bytes)) = (sample.identity(), sample.bytes()) {
                bridge.on_bus_sample(&bus, &identity, bytes);
            }
        }

        if last_health_log.elapsed() >= Duration::from_secs(10) {
            last_health_log = Instant::now();
            println!(
                "Foxglove bridge health: {} active client(s), {} channel(s) registered",
                bridge.client_count(),
                bridge.channel_count()
            );
        }

        let idle = if drained == 0 { 2 } else { 1 };
        thread::sleep(Duration::from_millis(idle));
    }

    println!("\nhypha_foxglove_bridge: shutting down...");
    ExitCode::SUCCESS
}
