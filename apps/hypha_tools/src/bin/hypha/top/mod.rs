//! `hypha top` -- interactive real-time bus monitor and node inspector (htop-style).

#![forbid(unsafe_code)]

mod graph;
mod graph_layout;
mod graph_ui;
mod inspector;
mod intercept;
mod intercept_ui;
mod state;
mod ui;

use crate::cli::{connect, stop_on_signals};
use clap::Parser;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use hypha_node::sdk::{BusNode, StreamFollower, StreamSelector};
use inspector::SchemaCache;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use state::{AppState, RATE_TICK, Tab};
use std::io::{self, Stdout, stdout};
use std::panic;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// RAII guard that restores the terminal on normal exit, signal, or panic.
struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = stdout();
        if let Err(e) = execute!(out, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(e);
        }

        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let mut out = stdout();
            let _ = execute!(out, LeaveAlternateScreen, Show);
            default_hook(info);
        }));

        Ok(Self { active: true })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let mut out = stdout();
            let _ = execute!(out, LeaveAlternateScreen, Show);
            self.active = false;
        }
    }
}

/// Command line of `hypha top`.
#[derive(Debug, Parser)]
#[command(
    name = "hypha top",
    about = "Full-screen bus monitor: topics, nodes, the Intercept tab, the graph."
)]
struct Cli {
    /// Directory the Intercept tab may read and write config files under; a path outside
    /// it is refused. Default: the working directory.
    #[arg(long = "intercept-dir", value_name = "dir")]
    intercept_dir: Option<PathBuf>,
}

/// The canonical directory the Intercept tab may use: the requested one, or the working
/// directory when none was requested.
///
/// # Errors
/// Returns the error when the directory does not exist or cannot be resolved.
fn resolve_intercept_dir(requested: Option<&Path>) -> io::Result<PathBuf> {
    let given = match requested {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    std::fs::canonicalize(given)
}

/// Runs the tool; `args` is the whole command line, `args[0]` the name it shows.
pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli = Cli::parse_from(&args);
    let intercept_dir = match resolve_intercept_dir(cli.intercept_dir.as_deref()) {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("hypha top: cannot use the intercept directory: {error}");
            return ExitCode::FAILURE;
        }
    };

    let stop_signal = match stop_on_signals("hypha top: ") {
        Ok(flag) => flag,
        Err(code) => return code,
    };
    let bus = match connect("hypha top: cannot connect to bus: ", "_hyphacli_top") {
        Ok(bus) => bus,
        Err(code) => return code,
    };

    // Every stream of every topic, each one as it appears: a second version of a topic joins
    // the table when its publisher starts.
    let mut follower =
        match StreamFollower::new(&bus, vec!["**".to_owned()], StreamSelector::default(), 0) {
            Ok(follower) => follower,
            Err(e) => {
                eprintln!("hypha top: cannot follow the bus: {e}");
                return ExitCode::FAILURE;
            }
        };
    if let Err(e) = follower.refresh(&bus) {
        eprintln!("hypha top: cannot list topics: {e}");
        return ExitCode::FAILURE;
    }

    let guard = match TerminalGuard::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("hypha top: failed to setup terminal: {e}");
            return ExitCode::FAILURE;
        }
    };

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            drop(guard);
            eprintln!("hypha top: failed to initialize ratatui: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut state = AppState::new();
    state.intercept.intercept_dir = Some(intercept_dir);
    let mut schema_cache = SchemaCache::new("hypha_top");

    let loop_result = run_tui_loop(
        &mut terminal,
        &bus,
        &mut follower,
        &mut state,
        &mut schema_cache,
        &stop_signal,
    );

    // The guard leaves the alternate screen, so the error is printed on the normal one.
    drop(guard);
    if let Err(error) = &loop_result {
        eprintln!("hypha top: {error}");
    }
    exit_code_for(&loop_result)
}

/// The exit code of the monitor's loop: success after a normal quit, failure when the
/// loop returned an error.
fn exit_code_for(loop_result: &io::Result<()>) -> ExitCode {
    match loop_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    bus: &BusNode,
    follower: &mut StreamFollower,
    state: &mut AppState,
    schema_cache: &mut SchemaCache,
    stop_signal: &Arc<AtomicBool>,
) -> io::Result<()> {
    let mut last_render = Instant::now();
    let mut last_rate_calc = Instant::now();
    let mut retry_schemas = std::collections::BTreeMap::new();
    let mut last_usage = Instant::now().checked_sub(USAGE_PERIOD).unwrap_or_else(Instant::now);

    while bus.ok() && !stop_signal.load(Ordering::Relaxed) {
        if let Err(error) = follower.refresh_if_changed(bus) {
            eprintln!("hypha top: {error}");
        }
        while let Some(received) = follower.next_sample().unwrap_or_else(|error| {
            eprintln!("hypha top: {error}");
            None
        }) {
            let (Some(identity), Some(bytes)) = (received.identity(), received.bytes()) else {
                continue;
            };
            state.on_sample(identity.topic, identity.schema_hash, bytes);
        }

        resolve_one_schema(bus, state, &mut retry_schemas);

        if last_rate_calc.elapsed() >= RATE_TICK {
            let dt_sec = last_rate_calc.elapsed().as_secs_f64();
            if !state.paused {
                state.calculate_rates(dt_sec);
            }
            last_rate_calc = Instant::now();
        }

        if last_usage.elapsed() >= USAGE_PERIOD {
            last_usage = Instant::now();
            let nodes = bus.list_nodes().map_err(io::Error::other)?;
            let pids: Vec<i32> =
                nodes.iter().filter(|node| node.is_alive).map(|node| node.pid).collect();
            state.proc_usage.sample(&pids, last_usage);
        }

        if last_render.elapsed() >= Duration::from_millis(30) {
            let listed = bus.list_nodes().map_err(io::Error::other)?;
            let nodes = state.track_nodes(listed);
            terminal.draw(|f| ui::draw(f, &mut *state, &nodes, bus, &mut *schema_cache))?;
            last_render = Instant::now();
        }

        if event::poll(Duration::from_millis(20))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && handle_key(state, key.code, key.modifiers)
        {
            return Ok(());
        }
    }

    Ok(())
}

/// How often node CPU and memory are sampled; CPU is the average over this window.
const USAGE_PERIOD: Duration = Duration::from_secs(1);

/// Resolves the schema of one shown topic that has none yet, without waiting.
fn resolve_one_schema(
    bus: &BusNode,
    state: &mut AppState,
    retry_schemas: &mut std::collections::BTreeMap<(String, u64), Instant>,
) {
    let Some((topic, schema_hash)) = state.unresolved_schema(retry_schemas, Instant::now()) else {
        return;
    };
    let key = (topic, schema_hash);
    match bus.resolve_schema_hash_now(&key.0, schema_hash) {
        Ok(Some(schema)) => {
            retry_schemas.remove(&key);
            state.apply_schema(&key.0, schema_hash, &schema.data_type, &schema.md5);
        }
        Ok(None) => {
            retry_schemas.insert(key, retry_deadline());
        }
        Err(error) => {
            eprintln!("hypha top: schema lookup failed: {error}");
            retry_schemas.insert(key, retry_deadline());
        }
    }
}

fn retry_deadline() -> Instant {
    Instant::now().checked_add(Duration::from_millis(100)).unwrap_or_else(Instant::now)
}

/// Handles one key press. Returns `true` when the application should quit.
fn handle_key(state: &mut AppState, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let ctrl_c =
        matches!(code, KeyCode::Char('c' | 'C')) && modifiers.contains(KeyModifiers::CONTROL);
    if ctrl_c {
        return true;
    }

    if state.show_inspector {
        handle_inspector_key(state, code);
        return false;
    }

    if state.search_focused && handle_search_key(state, code) {
        return false;
    }

    if state.active_tab == Tab::Intercept && !state.search_focused {
        let picked = state
            .visible_topics()
            .get(state.selected_topic)
            .map(|(stream, _)| stream.topic.clone());
        if state.intercept.handle_key(code, modifiers, &state.topics, picked.as_deref()) {
            return false;
        }
    }

    let graph_key = state.active_tab == Tab::Graph && !state.search_focused;
    if graph_key && state.graph.handle_key(code) {
        return false;
    }

    handle_global_key(state, code)
}

fn handle_inspector_key(state: &mut AppState, code: KeyCode) {
    if matches!(code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'Q')) {
        state.show_inspector = false;
    }
}

/// Consumes a key destined for the filter text box. Returns `true` when the key was
/// handled here and should not also be checked against the global bindings.
fn handle_search_key(state: &mut AppState, code: KeyCode) -> bool {
    match code {
        KeyCode::Enter | KeyCode::Esc => {
            state.search_focused = false;
            true
        }
        KeyCode::Backspace => {
            state.filter_query.pop();
            true
        }
        KeyCode::Char(c) if !is_global_binding_char(c) => {
            state.filter_query.push(c);
            true
        }
        _ => false,
    }
}

fn is_global_binding_char(c: char) -> bool {
    matches!(c, '1'..='5' | 's' | 'S' | ' ' | '/' | 'q' | 'Q')
}

/// Handles a key against the top-level bindings. Returns `true` when the application
/// should quit.
fn handle_global_key(state: &mut AppState, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('q' | 'Q') => return true,
        KeyCode::Char(digit @ '1'..='5') => {
            if let Some(tab) = Tab::from_digit(digit) {
                state.active_tab = tab;
            }
        }
        KeyCode::Char('s' | 'S') => state.sort_by = state.sort_by.next(),
        KeyCode::Char(' ') => state.paused = !state.paused,
        KeyCode::Char('/') => state.search_focused = true,
        KeyCode::Enter => {
            if state.active_tab == Tab::TopicsBus || state.active_tab == Tab::NodeRegistry {
                state.show_inspector = true;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => move_selection(state, -1),
        KeyCode::Down | KeyCode::Char('j') => move_selection(state, 1),
        KeyCode::PageUp => move_selection(state, -PAGE),
        KeyCode::PageDown => move_selection(state, PAGE),
        KeyCode::Home => move_selection(state, i32::MIN),
        KeyCode::End => move_selection(state, i32::MAX),
        KeyCode::Esc => {
            if !state.filter_query.is_empty() {
                state.filter_query.clear();
            }
            state.search_focused = false;
        }
        _ => {}
    }
    false
}

/// Moves the selection cursor for the active tab. The node list is only known at draw
/// time, so an unbounded increment here is clamped against the real count in `ui::draw`.
fn move_selection(state: &mut AppState, delta: i32) {
    let target = match state.active_tab {
        Tab::TopicsBus => &mut state.selected_topic,
        Tab::NodeRegistry => &mut state.selected_node,
        Tab::Intercept | Tab::Graph | Tab::Help => return,
    };
    let step = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
    if delta < 0 {
        *target = target.saturating_sub(step);
    } else {
        *target = target.saturating_add(step);
    }
}

/// Rows `PageUp`/`PageDown` move.
const PAGE: i32 = 10;

#[cfg(test)]
#[allow(clippy::expect_used)]
mod exit_tests {
    use super::*;

    #[test]
    fn a_normal_quit_exits_zero() {
        assert_eq!(exit_code_for(&Ok(())), ExitCode::SUCCESS);
    }

    #[test]
    fn a_failed_loop_exits_one() {
        let failed: io::Result<()> = Err(io::Error::other("cannot list nodes"));
        assert_eq!(exit_code_for(&failed), ExitCode::FAILURE);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod intercept_dir_tests {
    use super::*;

    #[test]
    fn the_default_intercept_directory_is_the_working_directory() {
        let working = std::env::current_dir().expect("the working directory");
        let resolved = resolve_intercept_dir(None).expect("the default directory");
        assert_eq!(resolved, working.canonicalize().expect("canonical working directory"));
    }

    #[test]
    fn a_requested_intercept_directory_is_canonical() {
        let requested = std::env::temp_dir().join(format!("hypha_top_dir_{}", std::process::id()));
        std::fs::create_dir_all(&requested).expect("a directory");
        let resolved = resolve_intercept_dir(Some(&requested)).expect("the requested directory");
        assert_eq!(resolved, requested.canonicalize().expect("canonical"));
        std::fs::remove_dir_all(&requested).expect("remove the directory");
    }

    #[test]
    fn a_missing_intercept_directory_is_an_error() {
        let missing =
            std::env::temp_dir().join(format!("hypha_top_missing_{}", std::process::id()));
        assert!(resolve_intercept_dir(Some(&missing)).is_err());
    }
}

#[cfg(test)]
mod schema_resolution_tests {
    use super::{AppState, resolve_one_schema};
    use hypha_node::sdk::BusNode;
    use hypha_testkit::unique_name as unique;
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    const BOOL_MD5: &str = "8b94c1b53db61fb6aed406028ad6332a";

    #[test]
    #[allow(clippy::expect_used)]
    fn schema_is_resolved_on_the_monitors_node_without_a_worker_thread() {
        let name = unique("top_schema_resolved");
        let topic = format!("/{name}/flag");
        let bus = BusNode::new(&name).expect("the monitor's bus");
        let _publisher = bus
            .publish_raw(&topic, "std_msgs/Bool", BOOL_MD5, "bool data\n")
            .expect("a publisher that writes its schema record");
        let schema_hash = hypha_node::sdk::schema_hash_from_md5(BOOL_MD5).expect("a valid md5");
        let mut state = AppState::new();
        state.on_sample(&topic, schema_hash, &[1]);
        let mut retry = BTreeMap::new();

        resolve_one_schema(&bus, &mut state, &mut retry);

        let shown = state.topics.get(&topic).expect("the topic is shown");
        assert_eq!(shown.data_type, "std_msgs/Bool");
        assert_eq!(shown.md5, BOOL_MD5);
        assert!(retry.is_empty());
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn an_unknown_schema_stays_unresolved_and_is_asked_again_later() {
        let name = unique("top_schema_unknown");
        let topic = format!("/{name}/nobody");
        let bus = BusNode::new(&name).expect("the monitor's bus");
        let schema_hash = hypha_node::sdk::schema_hash_from_md5(BOOL_MD5).expect("a valid md5");
        let mut state = AppState::new();
        state.on_sample(&topic, schema_hash, &[1]);
        let mut retry = BTreeMap::new();

        let started = Instant::now();
        resolve_one_schema(&bus, &mut state, &mut retry);
        let took = started.elapsed();

        // One read, no wait for a record: well inside one 30 ms render interval.
        assert!(took < Duration::from_millis(30), "the lookup took {took:?}");
        let shown = state.topics.get(&topic).expect("the topic is shown");
        assert!(shown.data_type.is_empty(), "no type is invented");
        assert!(retry.contains_key(&(topic, schema_hash)), "a retry is scheduled");
    }
}
