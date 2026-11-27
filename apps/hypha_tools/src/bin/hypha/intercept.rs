//! `hypha intercept` -- a headless man-in-the-middle relay.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use clap::Parser;
use hypha_node::sdk::message::EncodeInto as _;
use hypha_node::sdk::msgs::std_msgs::String as StringMessage;
use hypha_node::sdk::{
    AttachmentId, BusNode, Delivery, PublisherOptions, RawPublisher, StopReason, SubscriberOptions,
    WaitSet,
};
use hypha_tools::intercept_doc::{
    BackendState, InterceptConfig, InterceptionSpec, load_config, state_topic, valid_name,
};
use hypha_tools::intercept_relay::{Forward, Relay};

/// How long one run of the `WaitSet` lasts before the loop reloads, resolves and
/// reports.
const SLICE: Duration = Duration::from_millis(250);
/// The longest gap between two state documents.
const STATE_PERIOD: Duration = Duration::from_millis(500);
/// Frames an input queues between two drains: zero, the largest buffer of the input's
/// service policy depth, so a burst between two drains or across a slice boundary is not lost.
const INPUT_DEPTH: usize = 0;
const STRING_DEFINITION: &str = "string data\n";

/// Rewrite fields of any topic between a producer and its consumers.
#[derive(Debug, Parser)]
#[command(name = "hypha intercept")]
struct Cli {
    /// Backend name, `[a-z0-9_]+`. The state topic is `/hypha/intercept/<name>/state`.
    #[arg(long, default_value = "default")]
    name: String,
    /// The config file. It need not exist yet.
    #[arg(long, default_value = "intercept.toml")]
    config: PathBuf,
}

/// What the subscription callback and the loop share for one interception.
struct Entry {
    relay: Relay,
    publisher: Option<RawPublisher>,
    scratch: Vec<u8>,
}

struct Running {
    spec: InterceptionSpec,
    entry: Arc<Mutex<Entry>>,
    attachment: Option<AttachmentId>,
    /// True while the input has no publisher yet. The tool subscribes once it is
    /// listed, because a subscriber that declares no policy must not create it.
    waiting: bool,
}

struct Backend<'bus> {
    name: String,
    config_path: PathBuf,
    bus: &'bus BusNode,
    waitset: WaitSet<'bus>,
    running: Vec<Running>,
    applied_revision: u64,
    config_error: String,
    stamp: Option<(SystemTime, u64)>,
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli = Cli::parse_from(&args);
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hypha intercept: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    if !valid_name(&cli.name) {
        return Err(format!("backend name '{}' is not [a-z0-9_]+", cli.name));
    }
    let config_path = std::path::absolute(&cli.config)
        .map_err(|error| format!("cannot resolve {}: {error}", cli.config.display()))?;
    let bus = BusNode::new(&format!("hypha_intercept_{}", cli.name))?;
    let mut state_publisher = bus.publish_raw_configured(
        &state_topic(&cli.name),
        StringMessage::TYPE_NAME,
        StringMessage::SCHEMA_HASH,
        STRING_DEFINITION,
        PublisherOptions::default().latched(),
    )?;
    let mut backend = Backend {
        name: cli.name.clone(),
        config_path,
        bus: &bus,
        waitset: WaitSet::new(&bus)?,
        running: Vec::new(),
        applied_revision: 0,
        config_error: String::new(),
        stamp: None,
    };
    println!("hypha intercept: backend '{}' reads {}", backend.name, backend.config_path.display());

    let mut first = true;
    let mut last_state = String::new();
    let mut last_sent = Instant::now();
    while bus.ok() {
        backend.reload_if_changed(first);
        first = false;
        backend.resolve_pending();
        backend.attach_waiting();
        if last_sent.elapsed() >= STATE_PERIOD {
            backend.notice_schema_changes();
        }
        let state = toml::to_string(&backend.state()).map_err(|error| error.to_string())?;
        if state != last_state || last_sent.elapsed() >= STATE_PERIOD {
            publish_state(&mut state_publisher, &state)?;
            last_state = state;
            last_sent = Instant::now();
        }
        let result = backend.waitset.run_for(SLICE);
        if let Some(error) = result.err() {
            return Err(error);
        }
        if result.reason == StopReason::Requested {
            return Ok(());
        }
    }
    Ok(())
}

fn publish_state(publisher: &mut RawPublisher, text: &str) -> Result<(), String> {
    let message = StringMessage { data: text.to_owned() };
    let mut buffer = vec![0_u8; message.encoded_len()];
    message.encode_into(&mut buffer).map_err(|error| error.to_string())?;
    if !publisher.publish(&buffer) {
        eprintln!("hypha intercept: the state document was not published");
    }
    Ok(())
}

fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn lock(entry: &Mutex<Entry>) -> std::sync::MutexGuard<'_, Entry> {
    entry.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// True when the input's service is open on the bus, so a subscriber may read it.
fn input_open(bus: &BusNode, input: &str) -> Result<bool, String> {
    Ok(bus.list_topics()?.iter().any(|entry| entry.name == input))
}

/// Subscribes to `input` and attaches the subscription to `waitset`, relaying each frame
/// into `entry`.
fn attach_input<'bus>(
    bus: &'bus BusNode,
    waitset: &mut WaitSet<'bus>,
    input: &str,
    entry: &Arc<Mutex<Entry>>,
) -> Result<AttachmentId, String> {
    let options = SubscriberOptions::default().keeping(INPUT_DEPTH);
    let shared = Arc::clone(entry);
    let subscriber = bus
        .subscribe_raw_with_options(input, options)
        .map_err(|error| format!("cannot read {input}: {error}"))?;
    waitset.attach_subscription(subscriber, move |delivery| {
        relay_frame(&shared, delivery);
        Ok(())
    })
}

impl Backend<'_> {
    fn reload_if_changed(&mut self, force: bool) {
        let stamp = file_stamp(&self.config_path);
        if !force && stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        match load_config(&self.config_path).and_then(|config| self.apply(config)) {
            Ok(()) => self.config_error.clear(),
            Err(error) => {
                eprintln!("hypha intercept: config not applied: {error}");
                self.config_error = error;
            }
        }
    }

    /// Applies `config`. Every interception is checked before anything changes, so
    /// an invalid config leaves the running set as it was.
    fn apply(&mut self, config: InterceptConfig) -> Result<(), String> {
        let mut checked = Vec::with_capacity(config.interception.len());
        for spec in config.interception {
            let relay = Relay::new(&spec)?;
            checked.push((spec, relay));
        }
        let mut previous: BTreeMap<String, Running> =
            self.running.drain(..).map(|running| (running.spec.name.clone(), running)).collect();
        let mut next = Vec::with_capacity(checked.len());
        let mut errors = Vec::new();
        for (spec, relay) in checked {
            match previous.remove(&spec.name) {
                Some(mut running)
                    if running.spec.input == spec.input
                        && running.spec.output == spec.output
                        && !lock(&running.entry).relay.stopped() =>
                {
                    let updated = lock(&running.entry).relay.update(&spec);
                    if let Err(error) = updated {
                        errors.push(error);
                    }
                    running.spec = spec;
                    next.push(running);
                }
                replaced => {
                    if let Some(Err(error)) = replaced.map(|old| self.stop(old)) {
                        errors.push(error);
                    }
                    next.push(self.start(spec, relay));
                }
            }
        }
        for (_, removed) in previous {
            if let Err(error) = self.stop(removed) {
                errors.push(error);
            }
        }
        self.running = next;
        self.applied_revision = config.revision;
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }

    /// Subscribes every waiting interception whose input has been published.
    fn attach_waiting(&mut self) {
        for running in &mut self.running {
            if !running.waiting {
                continue;
            }
            if !matches!(input_open(self.bus, &running.spec.input), Ok(true)) {
                continue;
            }
            running.waiting = false;
            let attached =
                attach_input(self.bus, &mut self.waitset, &running.spec.input, &running.entry);
            match attached {
                Ok(id) => {
                    running.attachment = Some(id);
                    lock(&running.entry).relay.note_error(String::new());
                }
                Err(error) => {
                    lock(&running.entry).relay.fail(error);
                }
            }
        }
    }

    fn start(&mut self, spec: InterceptionSpec, relay: Relay) -> Running {
        let entry = Arc::new(Mutex::new(Entry { relay, publisher: None, scratch: Vec::new() }));
        if let Ok(false) = input_open(self.bus, &spec.input) {
            lock(&entry).relay.note_error(format!("waiting for '{}' to be published", spec.input));
            return Running { spec, entry, attachment: None, waiting: true };
        }
        let attached = attach_input(self.bus, &mut self.waitset, &spec.input, &entry);
        let attachment = match attached {
            Ok(id) => Some(id),
            Err(error) => {
                lock(&entry).relay.fail(error);
                None
            }
        };
        Running { spec, entry, attachment, waiting: false }
    }

    fn stop(&mut self, running: Running) -> Result<(), String> {
        match running.attachment {
            Some(id) => self.waitset.detach(id),
            None => Ok(()),
        }
    }

    /// Tells each attached relay which streams of its input have a publisher, so it sees a
    /// producer that went over to another schema. A failed listing is tried again at the
    /// next state period.
    fn notice_schema_changes(&self) {
        let topics = match self.bus.list_topics() {
            Ok(topics) => topics,
            Err(error) => {
                eprintln!("hypha intercept: cannot list the topics: {error}");
                return;
            }
        };
        for running in self.running.iter().filter(|running| running.attachment.is_some()) {
            let published: Vec<u64> = topics
                .iter()
                .filter(|topic| topic.name == running.spec.input && topic.published)
                .map(|topic| topic.schema_hash)
                .collect();
            lock(&running.entry).relay.input_published(&published);
        }
    }

    fn resolve_pending(&self) {
        for running in &self.running {
            let mut entry = lock(&running.entry);
            let Some(hash) = entry.relay.pending_schema() else { continue };
            match self.bus.resolve_schema_hash(&running.spec.input, hash) {
                Ok(Some(schema)) => {
                    if entry.relay.resolved(hash, &schema).is_err() {
                        continue;
                    }
                    let options = PublisherOptions::default().growable();
                    match self.bus.publish_raw_configured(
                        &running.spec.output,
                        &schema.data_type,
                        &schema.md5,
                        &schema.definition,
                        options,
                    ) {
                        Ok(publisher) => entry.publisher = Some(publisher),
                        Err(error) => entry
                            .relay
                            .fail(format!("cannot publish {}: {error}", running.spec.output)),
                    }
                }
                Ok(None) => {}
                Err(error) => entry.relay.note_error(format!("schema lookup failed: {error}")),
            }
        }
    }

    fn state(&self) -> BackendState {
        let interception =
            self.running.iter().map(|running| lock(&running.entry).relay.snapshot()).collect();
        BackendState {
            backend: self.name.clone(),
            config_path: self.config_path.display().to_string(),
            applied_revision: self.applied_revision,
            config_error: self.config_error.clone(),
            interception,
        }
    }
}

/// The subscription callback: one frame through the relay, then out.
fn relay_frame(shared: &Mutex<Entry>, delivery: &Delivery<'_>) {
    let mut guard = lock(shared);
    let Entry { relay, publisher, scratch } = &mut *guard;
    let Some(payload) = delivery.payload else {
        relay.note_error(format!("a frame on {} is not a hypha frame", delivery.identity.topic));
        return;
    };
    let Forward::Publish(bytes) = relay.on_frame(delivery.identity.schema_hash, payload, scratch)
    else {
        return;
    };
    let Some(publisher) = publisher.as_mut() else {
        relay.note_error("the output publisher is not open".to_owned());
        return;
    };
    if publisher.publish_at_framework_nanos(bytes, delivery.identity.framework_sent_nanos) {
        relay.count_published();
    } else {
        relay.note_error("the output publish was refused".to_owned());
    }
}
