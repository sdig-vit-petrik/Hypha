//! `hypha service` -- the services of the bus: list, info and call.
//!
//! `call` encodes the request from YAML or JSON against the definition the server
//! registered, calls with a blocking client of this tool's node (an observer slot, never a
//! declared client's), prints the response as `hypha topic echo` prints a message, and
//! exits with a code per outcome.

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use hypha_node::sdk::{
    BusNode, CallOutcome, ListedService, ServiceDefinitions, ServicePolicy, call_log_topic,
};
use hypha_tools::msg_reflect::{MessageSchema, decode_message};
use hypha_tools::msg_text::{encode_message, parse_text, template};

use crate::cli::{connect, parse_cli};

const TOOL: &str = "hypha service";

/// The exit code of a tool failure: a bad argument, a request that does not encode.
const EXIT_FAILURE: u8 = 1;

/// The services of the bus: list, info and call.
#[derive(Debug, Parser)]
#[command(
    name = "hypha service",
    after_help = "Exit codes of `call`: 0 answered, 1 the tool failed (arguments, a request \
                  that does not encode), 2 refused, 3 no server, 4 not sent, 5 server lost, \
                  6 timeout."
)]
struct Cli {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// One line per service: its name, type, MD5, server and clients.
    List,
    /// A service's type, server, clients, policy, call log and definitions.
    Info {
        /// The service, as `/camera/set_exposure`.
        service: String,
    },
    /// Call a service with a request written in YAML or JSON, and print the response.
    ///
    /// Without a request, print the request's template; a request of no fields is called.
    Call {
        /// The service, as `/camera/set_enabled`.
        service: String,
        /// The request: `"data: true"`, `'{data: true}'` or `'{"data": true}'`.
        request: Option<String>,
        /// Seconds to wait for the outcome; by default the service policy's timeout.
        #[arg(long, value_name = "s", value_parser = parse_seconds)]
        timeout: Option<Duration>,
    },
}

fn parse_seconds(text: &str) -> Result<Duration, String> {
    let seconds: f64 = text.parse().map_err(|_| format!("'{text}' is not a number of seconds"))?;
    Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| format!("'{text}' is not a duration above zero"))
}

/// The exit code of a call's outcome.
const fn exit_code(outcome: &CallOutcome) -> u8 {
    match outcome {
        CallOutcome::Answered(_) => 0,
        CallOutcome::Refused(_) => 2,
        CallOutcome::NoServer(_) => 3,
        CallOutcome::NotSent(_) => 4,
        CallOutcome::ServerLost(_) => 5,
        CallOutcome::Timeout(_) => 6,
    }
}

const EXIT_NO_SERVER: u8 = 3;

/// A service name as typed: absolute, or relative to the root.
fn resolved(service: &str) -> String {
    if service.starts_with('/') { service.to_owned() } else { format!("/{service}") }
}

fn short(md5: &str) -> &str {
    md5.get(..8).unwrap_or(md5)
}

fn server_text(service: &ListedService) -> String {
    match &service.server {
        None => "-".to_owned(),
        Some((name, _, _)) if name.is_empty() => "<unnamed>".to_owned(),
        Some((name, pid, true)) => format!("{name} (pid {pid})"),
        Some((name, pid, false)) => format!("{name} (pid {pid}, dead)"),
    }
}

fn policy_text(policy: Option<&ServicePolicy>) -> String {
    policy.map_or_else(
        || "<a stamp this build does not read>".to_owned(),
        |policy| {
            format!(
                "{}: {} clients, {} calls in flight each, request {} B, response {} B, timeout {} ms",
                policy.name,
                policy.max_clients,
                policy.max_outstanding,
                policy.max_request_bytes,
                policy.max_response_bytes,
                policy.timeout.as_millis()
            )
        },
    )
}

fn list(bus: &BusNode) -> Result<Vec<ListedService>, ExitCode> {
    bus.list_services().map_err(|cause| {
        eprintln!("{TOOL}: {cause}");
        ExitCode::from(EXIT_FAILURE)
    })
}

fn run_list(bus: &BusNode) -> ExitCode {
    let services = match list(bus) {
        Ok(services) => services,
        Err(code) => return code,
    };
    if services.is_empty() {
        println!("(no services)");
        return ExitCode::SUCCESS;
    }
    for service in &services {
        println!(
            "{:<32} {:<32} {}  server {}  clients {}",
            service.name,
            service.type_name,
            short(&service.md5),
            server_text(service),
            service.clients
        );
    }
    ExitCode::SUCCESS
}

/// The streams of `name`; the one with a live server first.
fn streams_of(services: Vec<ListedService>, name: &str) -> Vec<ListedService> {
    let mut found: Vec<ListedService> =
        services.into_iter().filter(|service| service.name == name).collect();
    found.sort_by_key(|service| !service.server.as_ref().is_some_and(|server| server.2));
    found
}

fn run_info(bus: &BusNode, service: &str) -> ExitCode {
    let name = resolved(service);
    let streams = match list(bus) {
        Ok(services) => streams_of(services, &name),
        Err(code) => return code,
    };
    if streams.is_empty() {
        eprintln!("{TOOL}: no service '{name}' on the bus: nothing serves or calls it");
        return ExitCode::from(EXIT_FAILURE);
    }
    for stream in &streams {
        let mut text = String::new();
        let _ = writeln!(text, "Service [{name}]");
        let _ = writeln!(text, "  type:     {}", stream.type_name);
        let _ = writeln!(text, "  md5:      {}", stream.md5);
        let _ = writeln!(text, "  server:   {}", server_text(stream));
        let _ = writeln!(text, "  clients:  {}", stream.clients);
        let _ = writeln!(text, "  policy:   {}", policy_text(stream.policy.as_ref()));
        let _ = writeln!(text, "  call log: {} (hypha_msgs/ServiceCall)", call_log_topic(&name));
        match bus.service_definitions(&name, &stream.md5) {
            Ok(Some(definitions)) => {
                for (label, type_name, definition) in [
                    ("request", &definitions.request_type, &definitions.request_definition),
                    ("response", &definitions.response_type, &definitions.response_definition),
                ] {
                    let _ = writeln!(text, "  {label} ({type_name}):");
                    for line in definition.lines() {
                        let _ = writeln!(text, "    {line}");
                    }
                }
            }
            Ok(None) => {
                let _ = writeln!(text, "  definitions: not registered (no live server gave them)");
            }
            Err(cause) => {
                let _ = writeln!(text, "  definitions: {cause}");
            }
        }
        print!("{text}");
    }
    ExitCode::SUCCESS
}

/// The stream to call and its definitions, waiting up to `wait` for a server that
/// registered them.
fn find_served(
    bus: &BusNode,
    name: &str,
    wait: Duration,
) -> Result<(ListedService, ServiceDefinitions), ExitCode> {
    let started = Instant::now();
    loop {
        let streams = streams_of(list(bus)?, name);
        for stream in &streams {
            let alive = stream.server.as_ref().is_some_and(|server| server.2);
            if !alive {
                continue;
            }
            match bus.service_definitions(name, &stream.md5) {
                Ok(Some(definitions)) => return Ok((stream.clone(), definitions)),
                Ok(None) => {
                    eprintln!(
                        "{TOOL}: the server {} of '{name}' registered no definitions, so the request cannot be encoded",
                        server_text(stream)
                    );
                    return Err(ExitCode::from(EXIT_FAILURE));
                }
                Err(cause) => {
                    eprintln!("{TOOL}: {cause}");
                    return Err(ExitCode::from(EXIT_FAILURE));
                }
            }
        }
        if started.elapsed() >= wait {
            let known = if streams.is_empty() {
                "nothing serves or calls it".to_owned()
            } else {
                let types: Vec<String> = streams
                    .iter()
                    .map(|stream| format!("{} ({})", stream.type_name, short(&stream.md5)))
                    .collect();
                format!("it is called as {}", types.join(", "))
            };
            eprintln!(
                "{TOOL}: no server: no live server of '{name}' within {} ms; {known}",
                wait.as_millis()
            );
            return Err(ExitCode::from(EXIT_NO_SERVER));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn schema(type_name: &str, definition: &str) -> Result<MessageSchema, ExitCode> {
    MessageSchema::from_definition(type_name, definition).map_err(|cause| {
        eprintln!("{TOOL}: the registered definition of {type_name} does not parse: {cause}");
        ExitCode::from(EXIT_FAILURE)
    })
}

fn run_call(
    bus: &BusNode,
    service: &str,
    request: Option<&str>,
    timeout: Option<Duration>,
) -> ExitCode {
    let name = resolved(service);
    // Before a server is seen, the policy's timeout is not known: wait the default 1 s.
    let wait = timeout.unwrap_or(ServicePolicy::default_service().timeout);
    let (stream, definitions) = match find_served(bus, &name, wait) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let mut request_schema =
        match schema(&definitions.request_type, &definitions.request_definition) {
            Ok(schema) => schema,
            Err(code) => return code,
        };
    let request_text = if let Some(text) = request {
        text
    } else {
        let shown = template(&mut request_schema, &definitions.request_type);
        if shown != "{}\n" {
            print!("{shown}");
            return ExitCode::SUCCESS;
        }
        ""
    };
    let encoded = parse_text(request_text)
        .and_then(|value| encode_message(&mut request_schema, &definitions.request_type, &value));
    let encoded = match encoded {
        Ok(bytes) => bytes,
        Err(cause) => {
            eprintln!("{TOOL}: the request is not a {}: {cause}", definitions.request_type);
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let client = match bus.service_client(
        &name,
        &stream.type_name,
        &stream.md5,
        stream.policy.clone(),
        timeout,
    ) {
        Ok(client) => client,
        Err(cause) => {
            eprintln!("{TOOL}: {cause}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let result = match client.call_and_wait(&encoded) {
        Ok(result) => result,
        Err(cause) => {
            eprintln!("{TOOL}: {cause}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let CallOutcome::Answered(response) = &result.outcome else {
        eprintln!("{TOOL}: {}", result.outcome);
        return ExitCode::from(exit_code(&result.outcome));
    };
    let mut response_schema =
        match schema(&definitions.response_type, &definitions.response_definition) {
            Ok(schema) => schema,
            Err(code) => return code,
        };
    let mut out = String::new();
    if !decode_message(&mut out, &definitions.response_type, &mut response_schema, response) {
        print!("{out}");
        eprintln!("{TOOL}: the response does not decode as {}", definitions.response_type);
        return ExitCode::from(EXIT_FAILURE);
    }
    if out.is_empty() {
        out.push_str("{}\n");
    }
    print!("{out}");
    ExitCode::SUCCESS
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli: Cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let bus = match connect("hypha service: ", "_hyphacli_service") {
        Ok(bus) => bus,
        Err(code) => return code,
    };
    match cli.verb {
        Verb::List => run_list(&bus),
        Verb::Info { service } => run_info(&bus, &service),
        Verb::Call { service, request, timeout } => {
            run_call(&bus, &service, request.as_deref(), timeout)
        }
    }
}
