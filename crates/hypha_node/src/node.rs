//! The session handle a caller holds.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::status::Error;

/// A session on the bus. Opaque to the caller.
///
/// `hypha_node_create` reads `HYPHA_USE_SIM_TIME`: `1` or `true` puts the node on
/// simulated time, in the replay lockstep unless `HYPHA_REPLAY_LOCKSTEP=0` turns it off;
/// `0`, `false` or an unset variable leaves it off, and any other value is
/// `InvalidArgument`. A refused parameter file or `[log]` table is `InvalidArgument`,
/// or `InvalidState` for a sink lock another process holds or a second, different `[log]`
/// table. Each `WaitSet` on the node co-owns it, so the node outlives it whatever order
/// (or thread) the caller destroys them in.
pub struct HyphaNode {
    /// The node name: from code, or from the `node.name` parameter.
    pub(crate) name: String,
    pub(crate) parameters: crate::config::HyphaConfig,
    pub(crate) running: AtomicBool,
    /// The node's own execution configuration, created on first use. Released before
    /// `transport` drops the underlying iceoryx2 node.
    pub(crate) default_waitset: std::sync::Mutex<crate::waitset::WaitSetSlot>,
    /// Framework-owned `/clock` input when this node uses simulated time. Declared before
    /// `transport` so it drops before the iceoryx2 node.
    pub(crate) sim_clock_subscriber:
        std::sync::Mutex<Option<Box<crate::subscriber::HyphaSubscriber>>>,
    /// Framework-owned `/clock_progress` input when this node uses simulated time.
    /// Declared before `transport`.
    pub(crate) progress_subscriber:
        std::sync::Mutex<Option<Box<crate::subscriber::HyphaSubscriber>>>,
    /// The newest replay progress each other node has reported.
    pub(crate) upstream: std::sync::Mutex<hypha_runtime::upstream_reports::UpstreamReports>,
    /// The replay progress publisher and what it reported last. Declared before `transport`.
    pub(crate) progress: std::sync::Mutex<crate::node_progress::NodeProgress>,
    pub(crate) uses_sim_time: bool,
    /// On simulated time in an open-loop playback (`HYPHA_REPLAY_LOCKSTEP=0`): the node
    /// reports no progress, so no player or node waits for it, it holds no event for
    /// another node's report, and a replay input that lost samples is warned of instead of
    /// failing the run.
    pub(crate) open_loop: bool,
    /// Committed cycles of this node. A node with a clock driver runs no other cycle.
    pub(crate) committed_cycles: std::sync::atomic::AtomicUsize,
    /// True while a committed cycle of this node drives `/clock`.
    pub(crate) drives_clock: std::sync::atomic::AtomicBool,
    pub(crate) schema: crate::schema::SchemaState,
    /// Execution counters every `WaitSet` on this node adds to.
    pub(crate) health: std::sync::Arc<crate::node_health::ExecutionHealth>,
    /// The checks the application reports about itself, for `~/state`.
    pub(crate) checks: std::sync::Mutex<crate::node_checks::NodeChecks>,
    /// The rows of the node introspection. Endpoints keep a handle to it.
    pub(crate) introspection: std::sync::Arc<crate::introspection::IntrospectionRegistry>,
    /// Set once by [`node_destroy`]: the node has left the bus. Every publisher of the node
    /// holds a share and sends nothing afterwards (#97).
    pub(crate) left: std::sync::Arc<AtomicBool>,
    /// The streams of the topics this node subscribes to, refreshed by its housekeeping.
    pub(crate) topic_streams: crate::topic_streams::Registry,
    /// Declared last so every port owned by `HyphaNode` drops before the iceoryx2 node.
    pub(crate) transport: hypha_iceoryx2::Iox2Transport,
}

/// The environment variable that switches a node to simulated time; a parameter file
/// cannot turn simulated time on or off.
const SIM_TIME_VARIABLE: &str = "HYPHA_USE_SIM_TIME";

/// The line written once per process when a node starts on simulated time.
const SIM_TIME_WARNING: &str =
    "simulated time is on (HYPHA_USE_SIM_TIME): this node follows /clock";

static SIM_TIME_WARNED: AtomicBool = AtomicBool::new(false);

/// Whether the environment asks for simulated time.
///
/// An unset variable means host time. Any other value than `1`, `true`, `0` or `false` is
/// an error: a wrong deployment setting must stop the node.
fn sim_time_requested() -> Result<bool, String> {
    let text = match std::env::var(SIM_TIME_VARIABLE) {
        Ok(text) => text,
        Err(std::env::VarError::NotPresent) => return Ok(false),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{SIM_TIME_VARIABLE} is not readable text"));
        }
    };
    parse_sim_time(&text)
}

/// The environment variable that turns the replay lockstep off for a whole launch: the
/// player and every node read it (#102).
const LOCKSTEP_VARIABLE: &str = "HYPHA_REPLAY_LOCKSTEP";

/// Whether the environment keeps the replay lockstep: an unset variable, `1` or `true`
/// keep it, `0` or `false` turn it off. Any other value is an error, as for
/// `HYPHA_USE_SIM_TIME`.
fn lockstep_requested() -> Result<bool, String> {
    match std::env::var(LOCKSTEP_VARIABLE) {
        Ok(text) => parse_lockstep(&text),
        Err(std::env::VarError::NotPresent) => Ok(true),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{LOCKSTEP_VARIABLE} is not readable text"))
        }
    }
}

fn parse_lockstep(text: &str) -> Result<bool, String> {
    match text {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!("{LOCKSTEP_VARIABLE} must be 1, true, 0 or false, not '{text}'")),
    }
}

fn parse_sim_time(text: &str) -> Result<bool, String> {
    match text {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        "follow" => Err(format!(
            "{SIM_TIME_VARIABLE}=follow is gone: set {SIM_TIME_VARIABLE}=1 and, for a playback \
             outside the lockstep, {LOCKSTEP_VARIABLE}=0 for the whole launch"
        )),
        _ => Err(format!("{SIM_TIME_VARIABLE} must be 1, true, 0 or false, not '{text}'")),
    }
}

fn warn_sim_time_once() {
    let already_warned = SIM_TIME_WARNED.swap(true, Ordering::AcqRel);
    if already_warned {
        return;
    }
    log::warn!(target: "hypha", "{SIM_TIME_WARNING}");
}

/// The framework inputs of simulated time: `/clock`, and `/clock_progress`, the reports of
/// the other nodes. A node on simulated time has both, and any other node has none.
type SimInputs = (
    Option<Box<crate::subscriber::HyphaSubscriber>>,
    Option<Box<crate::subscriber::HyphaSubscriber>>,
);

fn create_sim_inputs(
    transport: &hypha_iceoryx2::Iox2Transport,
    introspection: &std::sync::Arc<crate::introspection::IntrospectionRegistry>,
    use_sim_time: bool,
    open_loop: bool,
) -> Result<SimInputs, Error> {
    if !use_sim_time {
        return Ok((None, None));
    }
    let clock = crate::subscriber::create_sim_clock_subscriber(transport, introspection)?;
    // Outside the lockstep no report is read: the node waits for no other node.
    if open_loop {
        return Ok((Some(clock), None));
    }
    let progress = crate::subscriber::create_progress_subscriber(transport, introspection)?;
    Ok((Some(clock), Some(progress)))
}

/// Log one startup error and return the error that node creation reports for it.
fn report_invalid(cause: &str) -> Error {
    log::error!(target: "hypha", "hypha: {cause}");
    Error::InvalidArgument
}

/// The `node.name` parameter, or `None` when the file does not set it.
pub(crate) fn configured_node_name(
    parameters: &hypha_core::parameters::Parameters,
) -> Result<Option<&str>, Error> {
    match parameters.value("node.name") {
        None => Ok(None),
        Some(hypha_core::parameters::ParamValue::Text(text)) => Ok(Some(text.as_str())),
        Some(_) => {
            log::error!(target: "hypha", "hypha: the parameter node.name must be text");
            Err(Error::InvalidArgument)
        }
    }
}

/// Refuse a name that is neither a node name nor a CLI tool name.
pub(crate) fn check_node_name(node_name: &str) -> Result<(), Error> {
    let is_valid_name =
        hypha_core::port::is_cli_node_name(node_name) || hypha_core::port::is_node_name(node_name);
    if !is_valid_name {
        log::error!(target: "hypha", "hypha: '{node_name}' is not a node name: use [a-z][a-z0-9_], at most {} characters, \
                 or {}<name> for a CLI tool", hypha_core::port::MAX_NODE_NAME_LENGTH, hypha_core::port::CLI_NODE_PREFIX);
        return Err(Error::InvalidArgument);
    }
    Ok(())
}

/// The node name: the `node.name` parameter when the file sets it, else the name from
/// code. Refuses an invalid name, and one an alive node of another process in this
/// domain holds; a CLI tool name is never held.
fn effective_node_name(
    code_name: &str,
    parameters: &hypha_core::parameters::Parameters,
) -> Result<String, Error> {
    let node_name = configured_node_name(parameters)?.unwrap_or(code_name);
    check_node_name(node_name)?;
    let is_cli_name = hypha_core::port::is_cli_node_name(node_name);
    if is_cli_name {
        return Ok(node_name.to_string());
    }
    // A node of this process can stay in the registry until its last port is
    // released, so the check looks at other processes only.
    let own_pid = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
    let registered = hypha_iceoryx2::list_nodes().map_err(Error::from)?;
    let name_is_taken = registered
        .iter()
        .any(|node| node.is_alive && node.pid != own_pid && node.name == node_name);
    if name_is_taken {
        log::error!(target: "hypha", "hypha: an alive node called '{node_name}' already runs in this domain");
        return Err(Error::InvalidState);
    }
    Ok(node_name.to_string())
}

fn log_error(cause: &hypha_log::LogError) -> Error {
    match cause {
        hypha_log::LogError::InvalidArgument(_) => Error::InvalidArgument,
        hypha_log::LogError::InvalidState(_) => Error::InvalidState,
        hypha_log::LogError::ResourceLimit => Error::ResourceLimit,
    }
}

impl HyphaNode {
    /// True while the node has not been asked to stop, by `hypha_node_shutdown` or by
    /// a SIGINT or SIGTERM of the process.
    pub(crate) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire) && !crate::process_stop::stop_requested()
    }

    /// True when this node's timers and cycles run on simulated time: its own `/clock`,
    /// or a clock the process was put on.
    pub(crate) fn on_sim_time(&self) -> bool {
        self.uses_sim_time || hypha_time::is_sim_time()
    }

    /// Resolves a topic or service name: `~/x` becomes `/<node>/x`.
    ///
    ///
    /// A CLI tool node has no `~/` names: every process of the tool holds its name.
    pub(crate) fn resolve_name(&self, name: &str) -> Result<String, Error> {
        let is_node_scoped = name.starts_with('~');
        let is_cli_node = hypha_core::port::is_cli_node_name(&self.name);
        let scoped_name_of_cli_node = is_node_scoped && is_cli_node;
        if scoped_name_of_cli_node {
            log::error!(target: "hypha", "hypha: '{name}' is refused: the CLI tool node '{}' has no `~/` names", self.name);
            return Err(Error::InvalidTopic);
        }
        hypha_core::port::resolve_node_name(name, &self.name).map_err(Error::from)
    }
}

/// The prepared configuration when the caller gave one, else one loaded from `parameter_file`.
fn node_config(
    parameter_file: Option<&str>,
    prepared: Option<&crate::config::HyphaConfig>,
) -> Result<crate::config::HyphaConfig, Error> {
    let Some(config) = prepared else {
        return crate::config::load_config(parameter_file);
    };
    let same_path = parameter_file
        .is_none_or(|path| config.parameter_file.as_deref().and_then(Path::to_str) == Some(path));
    if !same_path {
        let loaded = config
            .parameter_file
            .as_deref()
            .map_or_else(|| "no file".to_owned(), |path| format!("'{}'", path.display()));
        let given = parameter_file.unwrap_or_default();
        log::error!(target: "hypha", "hypha: node creation refused: the configuration was loaded from {loaded}, not '{given}'");
        return Err(Error::InvalidArgument);
    }
    Ok(config.clone())
}

/// Record the parameter file as the `parameter_file` row, source command line.
fn record_parameter_file_row(
    introspection: &crate::introspection::IntrospectionRegistry,
    file: Option<&str>,
) {
    let Some(path) = file else {
        return;
    };
    introspection.record_config(hypha_core::introspection::ConfigRow {
        key: "parameter_file".to_owned(),
        value: path.to_owned(),
        default: None,
        source: hypha_core::introspection::ConfigSource::CommandLine,
    });
}

/// The node name: the `node.name` parameter when the parameter file sets it, else the
/// name the node was created with.
#[must_use]
pub fn node_name(node: &HyphaNode) -> &str {
    &node.name
}

/// Create a node called `name`. `config` supplies the parameters; `parameter_file` must
/// then name its file. Without `config` the node loads `parameter_file`, or no file.
///
/// `HYPHA_REPLAY_LOCKSTEP=0` (or `false`) puts a node that `HYPHA_USE_SIM_TIME=1` puts in
/// the replay lockstep outside it: a ROS-style open-loop playback (#102).
///
/// # Errors
/// `InvalidArgument` for an invalid name, file, parameter, `HYPHA_USE_SIM_TIME` or
/// `HYPHA_REPLAY_LOCKSTEP`;
/// `InvalidState` for a name an alive node holds; the transport's error otherwise.
pub fn node_create(
    name: &str,
    parameter_file: Option<&str>,
    config: Option<&crate::config::HyphaConfig>,
) -> Result<Arc<HyphaNode>, Error> {
    hypha_log::install();
    let mut config = node_config(parameter_file, config)?;
    let parameters = &*config;
    let parameter_path = config.parameter_file.as_deref();
    let lockstep = lockstep_requested().map_err(|cause| report_invalid(&cause))?;
    let use_sim_time = sim_time_requested().map_err(|cause| report_invalid(&cause))?;
    let open_loop = use_sim_time && !lockstep;
    let node_name = effective_node_name(name, parameters)?;
    let transport = hypha_iceoryx2::Iox2Transport::with_name(&node_name).map_err(|cause| {
        log::error!(target: "hypha", "hypha: node creation refused: the iceoryx2 node '{node_name}' cannot be opened: {cause:?}");
        Error::from(cause)
    })?;
    let created_wall_nanos = hypha_time::wall_now().nanos();
    let introspection = crate::introspection::IntrospectionRegistry::shared(
        crate::introspection::identity(&node_name, created_wall_nanos),
    );
    record_parameter_file_row(&introspection, parameter_path.and_then(Path::to_str));
    for row in config.introspection_rows() {
        introspection.record_config(row);
    }
    let (sim_clock_subscriber, progress_subscriber) =
        create_sim_inputs(&transport, &introspection, use_sim_time, open_loop)?;
    // Prepared here, not above: a prepared configuration holds the log's prepare gate
    // until it is committed, and the steps before this line wait on other processes.
    let prepared_log = match (parameters.log_table(), parameter_path) {
        (Some(table), Some(file)) => match hypha_log::prepare(table, Path::new(file)) {
            Ok(prepared) => Some(prepared),
            Err(cause) => {
                // The log engine names an invalid table itself; a full module table it does not.
                if cause == hypha_log::LogError::ResourceLimit {
                    log::error!(target: "hypha", "hypha: node creation refused: the [log] table: {cause:?}");
                }
                return Err(log_error(&cause));
            }
        },
        _ => None,
    };
    if let Some(prepared) = prepared_log
        && let Err(cause) = hypha_log::commit(prepared)
    {
        log::error!(target: "hypha", "hypha: {cause}");
        return Err(log_error(&cause));
    }
    // Written after the commit, so the configured console and sinks receive it.
    if use_sim_time {
        warn_sim_time_once();
    }
    for replaced in parameters.overrides() {
        log::debug!(
            target: "hypha",
            "hypha: {} from {} overrides {}",
            replaced.key,
            replaced.winner.display(),
            replaced.overridden.display()
        );
    }
    // A key ignored in code is logged and recorded where it is ignored; one the file lists
    // is written here, after the log commit.
    for listed in parameters.ignored().iter().filter(|entry| entry.listed_in_file) {
        log::info!(target: "hypha", "hypha: parameter '{}' is not read on purpose: {}", listed.key, listed.reason);
        introspection.record_config(crate::config::ignored_row(&listed.key, &listed.reason));
    }
    config.node_introspection = Some(std::sync::Arc::clone(&introspection));
    // The first /clock value, not this zero, anchors the replay cycles. A later sim node
    // must not reset another node's time.
    let placeholder_needed = use_sim_time && !hypha_time::is_sim_time();
    if placeholder_needed
        && !hypha_time::enable_sim_time_placeholder(hypha_time::Time::from_nanos(0))
    {
        return Err(Error::InvalidState);
    }
    let node = HyphaNode {
        name: node_name,
        parameters: config,
        running: AtomicBool::new(true),
        default_waitset: std::sync::Mutex::new(crate::waitset::WaitSetSlot::Absent),
        sim_clock_subscriber: std::sync::Mutex::new(sim_clock_subscriber),
        progress_subscriber: std::sync::Mutex::new(progress_subscriber),
        upstream: std::sync::Mutex::new(hypha_runtime::upstream_reports::UpstreamReports::default()),
        progress: std::sync::Mutex::new(crate::node_progress::NodeProgress::default()),
        uses_sim_time: use_sim_time,
        open_loop,
        committed_cycles: std::sync::atomic::AtomicUsize::new(0),
        drives_clock: std::sync::atomic::AtomicBool::new(false),
        schema: crate::schema::SchemaState::default(),
        health: std::sync::Arc::default(),
        checks: std::sync::Mutex::default(),
        introspection,
        left: std::sync::Arc::default(),
        topic_streams: crate::topic_streams::Registry::default(),
        transport,
    };
    // A process that a backward seek restarted waits here, before any port of its own.
    if use_sim_time && !open_loop {
        crate::replay_restart::rejoin(&node)?;
    }
    // Only cycles take part in the replay: a cycle's commit opens the progress publisher
    // (`cycle/registration.rs`), so a node with only plain callbacks is on no one's list.
    hypha_sys::signal::install();
    Ok(Arc::new(node))
}

/// Stop `node` and release its default `WaitSet`, with its cycles and every registration
/// context; its publishers refuse every later loan and send with `InvalidState`.
/// Simulated time outlives the node, so the other nodes keep their clock.
pub fn node_destroy(node: &HyphaNode) {
    node.running.store(false, Ordering::Release);
    node.left.store(true, Ordering::Release);
    // The default `WaitSet` owns a share of the node, so it goes first.
    let default_waitset = crate::locked(&node.default_waitset).close();
    drop(default_waitset);
}

/// True while the node has not been asked to stop, by [`node_shutdown`] or by a SIGINT
/// or SIGTERM of the process.
#[must_use]
pub fn node_ok(node: &HyphaNode) -> bool {
    node.is_running()
}

/// Ask the node to stop.
pub fn node_shutdown(node: &HyphaNode) {
    node.running.store(false, Ordering::Release);
}

#[cfg(test)]
mod sim_time_tests {
    use super::{parse_lockstep, parse_sim_time};

    #[test]
    fn follow_is_refused_and_names_the_open_loop_switch() {
        let refusal = parse_sim_time("follow");
        let names_the_switch =
            refusal.as_ref().is_err_and(|text| text.contains("HYPHA_REPLAY_LOCKSTEP=0"));
        assert!(names_the_switch, "{refusal:?}");
    }

    #[test]
    fn simulated_time_is_on_or_off_and_other_text_is_refused() {
        assert_eq!(parse_sim_time("1"), Ok(true));
        assert_eq!(parse_sim_time("true"), Ok(true));
        assert_eq!(parse_sim_time("0"), Ok(false));
        assert_eq!(parse_sim_time("false"), Ok(false));
        assert!(parse_sim_time("Follow").is_err());
        assert!(parse_sim_time("yes").is_err());
    }

    #[test]
    fn the_lockstep_is_on_unless_turned_off_and_other_text_is_refused() {
        assert_eq!(parse_lockstep("1"), Ok(true));
        assert_eq!(parse_lockstep("true"), Ok(true));
        assert_eq!(parse_lockstep("0"), Ok(false));
        assert_eq!(parse_lockstep("false"), Ok(false));
        assert!(parse_lockstep("off").is_err());
        assert!(parse_lockstep("").is_err());
    }
}
