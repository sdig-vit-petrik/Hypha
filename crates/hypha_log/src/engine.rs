//! The process-wide engine: module table, configuration, targets, counters.

use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::ThreadId;

use crate::config::{self, Config};
use crate::console::{Console, ConsoleOutcome};
use crate::file::{self, LogFile};
use crate::level::Level;
use crate::render::render_line;
use crate::{Health, LogError, ModuleId};

pub(crate) const MAX_MODULES: usize = 256;
const UNREGISTERED: u32 = Level::Off as u32 + 1;

struct Slot {
    name: OnceLock<String>,
    level: AtomicU32,
}

struct Active {
    config: Config,
    sinks: Vec<LogFile>,
}

#[derive(Default)]
struct Counters {
    lines_written: AtomicU64,
    write_failures: AtomicU64,
    rejected_lines: AtomicU64,
    console_lines_dropped: AtomicU64,
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// The modules and the configuration of one process.
pub(crate) struct Engine {
    slots: [Slot; MAX_MODULES],
    /// Number of registered modules. The lock orders registration against a commit.
    registered: Mutex<usize>,
    lowest_target: AtomicU32,
    defaults: Config,
    active: OnceLock<Active>,
    /// Held from `prepare` until the prepared table is committed or dropped.
    prepare_gate: PrepareGate,
    console_is_terminal: OnceLock<bool>,
    console: Console,
    /// Set once the file sinks were told that the console cannot be written.
    console_unavailable_reported: AtomicBool,
    counters: Counters,
}

/// Lock `mutex`, recovering a poisoned guard: a panic elsewhere must not stop logging.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Serialises `prepare` with the commit of its result. Reentrant on one thread.
#[derive(Debug)]
pub(crate) struct PrepareGate {
    state: Mutex<GateState>,
    released: Condvar,
}

#[derive(Debug, Default)]
struct GateState {
    owner: Option<ThreadId>,
    depth: usize,
}

impl PrepareGate {
    const fn new() -> Self {
        Self { state: Mutex::new(GateState { owner: None, depth: 0 }), released: Condvar::new() }
    }

    fn enter(&'static self) -> GateGuard {
        let me = std::thread::current().id();
        let mut state = lock(&self.state);
        while state.owner.is_some_and(|owner| owner != me) {
            state = self.released.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        state.owner = Some(me);
        state.depth = state.depth.saturating_add(1);
        GateGuard { gate: self }
    }
}

/// Holds [`PrepareGate`] until dropped.
#[derive(Debug)]
pub(crate) struct GateGuard {
    gate: &'static PrepareGate,
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.gate.state);
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0 {
            state.owner = None;
            self.gate.released.notify_all();
        }
    }
}

impl Engine {
    fn new() -> Engine {
        let defaults = Config::default();
        let engine = Engine {
            slots: [const { Slot { name: OnceLock::new(), level: AtomicU32::new(UNREGISTERED) } };
                MAX_MODULES],
            registered: Mutex::new(0),
            lowest_target: AtomicU32::new(defaults.lowest_target_level() as u32),
            defaults,
            active: OnceLock::new(),
            prepare_gate: PrepareGate::new(),
            console_is_terminal: OnceLock::new(),
            console: Console::new(),
            console_unavailable_reported: AtomicBool::new(false),
            counters: Counters::default(),
        };
        // The module "default" is always id 0.
        let _ = engine.register("default");
        engine
    }

    fn config(&self) -> &Config {
        self.active.get().map_or(&self.defaults, |active| &active.config)
    }

    /// The file sinks of the active configuration; none before a commit.
    fn sinks(&self) -> &[LogFile] {
        self.active.get().map_or(&[][..], |active| active.sinks.as_slice())
    }

    /// True when the console is on and a line at `level` reaches it.
    fn console_wants(&self, level: Level) -> bool {
        let config = self.config();
        config.console && level >= config.console_level
    }

    /// True when the `[log]` table names the module.
    pub(crate) fn sets_module(&self, name: &str) -> bool {
        self.config().sets_module(name)
    }

    fn slot(&self, module: ModuleId) -> Option<(&Slot, u32)> {
        let slot = self.slots.get(usize::try_from(module.0).ok()?)?;
        let level = slot.level.load(Ordering::Acquire);
        (level != UNREGISTERED).then_some((slot, level))
    }

    pub(crate) fn register(&self, name: &str) -> Result<ModuleId, LogError> {
        if !config::valid_module_name(name) {
            return Err(LogError::InvalidArgument(format!("invalid module name {name:?}")));
        }
        let mut count = lock(&self.registered);
        let known = self
            .slots
            .iter()
            .take(*count)
            .position(|slot| slot.name.get().is_some_and(|existing| existing == name));
        if let Some(index) = known {
            return Ok(ModuleId(u32::try_from(index).unwrap_or(u32::MAX)));
        }
        let slot = self.slots.get(*count).ok_or(LogError::ResourceLimit)?;
        let level = self.config().module_level(name);
        let _ = slot.name.set(name.to_string());
        slot.level.store(level as u32, Ordering::Release);
        let id = u32::try_from(*count).unwrap_or(u32::MAX);
        *count = count.wrapping_add(1);
        Ok(ModuleId(id))
    }

    pub(crate) fn enabled(&self, module: ModuleId, level: Level) -> bool {
        if level == Level::Off {
            return false;
        }
        let Some((_, module_level)) = self.slot(module) else { return false };
        let threshold = module_level.max(self.lowest_target.load(Ordering::Relaxed));
        level as u32 >= threshold
    }

    pub(crate) fn count_rejected(&self) {
        bump(&self.counters.rejected_lines);
    }

    pub(crate) fn health(&self) -> Health {
        let counters = &self.counters;
        Health {
            lines_written: counters.lines_written.load(Ordering::Relaxed),
            write_failures: counters.write_failures.load(Ordering::Relaxed),
            rejected_lines: counters.rejected_lines.load(Ordering::Relaxed),
            console_lines_dropped: counters.console_lines_dropped.load(Ordering::Relaxed),
        }
    }

    fn console_colour(&self) -> bool {
        *self.console_is_terminal.get_or_init(|| std::io::stderr().is_terminal())
    }

    pub(crate) fn write(&self, module: ModuleId, level: Level, text: &[u8]) {
        let slot = self.slot(module);
        let (Some((slot, module_level)), true) = (slot, level != Level::Off) else {
            self.count_rejected();
            return;
        };
        if (level as u32) < module_level {
            return;
        }
        let name = slot.name.get().map_or("", String::as_str);
        let to_console = self.console_wants(level);
        let sinks = self.sinks();
        let to_file = sinks.iter().any(|sink| level >= sink.level());
        if !to_console && !to_file {
            return;
        }
        let line = render_line(level, name, text);
        let mut delivered = false;
        // A failed roll is reported after the loop, once every sink lock is released.
        let mut roll_warnings: Vec<(usize, String)> = Vec::new();
        for (index, target) in sinks.iter().enumerate() {
            if level < target.level() {
                continue;
            }
            match target.write_line(&line.bytes) {
                file::WriteOutcome::Written => delivered = true,
                file::WriteOutcome::WrittenRollFailed { first_of_period, error } => {
                    delivered = true;
                    bump(&self.counters.write_failures);
                    if first_of_period {
                        let warning = format!(
                            "hypha_log: cannot roll {}: {error}; retrying every {} s",
                            target.path().display(),
                            file::ROLL_RETRY.as_secs()
                        );
                        roll_warnings.push((index, warning));
                    }
                }
                file::WriteOutcome::Failed => bump(&self.counters.write_failures),
            }
        }
        if to_console {
            delivered |= self.write_console(level, &line);
        }
        if delivered {
            bump(&self.counters.lines_written);
        }
        for (failing, warning) in &roll_warnings {
            self.write_roll_warning(sinks, *failing, warning);
        }
    }

    /// Write `line` to standard error without blocking. True when all of it was written.
    fn write_console(&self, level: Level, line: &crate::render::Line) -> bool {
        let coloured;
        let bytes = if self.console_colour() {
            coloured = line.coloured(level);
            &coloured
        } else {
            &line.bytes
        };
        let outcome = self.console.write_line(bytes);
        if outcome == ConsoleOutcome::Written {
            return true;
        }
        bump(&self.counters.write_failures);
        bump(&self.counters.console_lines_dropped);
        self.report_unavailable_console();
        false
    }

    /// Tell the file sinks once that standard error cannot be written without blocking.
    fn report_unavailable_console(&self) {
        if !self.console.unavailable() {
            return;
        }
        let already = self.console_unavailable_reported.swap(true, Ordering::AcqRel);
        if already {
            return;
        }
        let sinks = self.sinks();
        let text = b"hypha_log: standard error cannot be written without blocking; console lines are dropped";
        let line = render_line(Level::Warn, "hypha", text);
        for target in sinks.iter().filter(|sink| Level::Warn >= sink.level()) {
            if target.write_line(&line.bytes) == file::WriteOutcome::Failed {
                bump(&self.counters.write_failures);
            }
        }
    }

    /// Write one message of the engine itself to the console, at `ERROR`.
    fn write_console_message(&self, text: &str) {
        let line = render_line(Level::Error, "hypha", text.as_bytes());
        let _written = self.write_console(Level::Error, &line);
    }

    /// Write the warning of a failed roll to every target but the sink whose roll
    /// failed. Called with no sink lock held.
    fn write_roll_warning(&self, sinks: &[LogFile], failing: usize, warning: &str) {
        let line = render_line(Level::Warn, "hypha", warning.as_bytes());
        let mut delivered = false;
        for (index, target) in sinks.iter().enumerate() {
            let skipped = index == failing || Level::Warn < target.level();
            if skipped {
                continue;
            }
            let outcome = target.write_line(&line.bytes);
            if outcome == file::WriteOutcome::Failed {
                bump(&self.counters.write_failures);
            } else {
                delivered = true;
            }
        }
        if self.console_wants(Level::Warn) {
            delivered |= self.write_console(Level::Warn, &line);
        }
        if delivered {
            bump(&self.counters.lines_written);
        }
    }

    /// Make `config` the process configuration. The first commit wins. A later commit
    /// of an equal table gives `Ok`; a different table gives `InvalidState`.
    fn install(&self, config: Config, sinks: Vec<LogFile>) -> Result<(), LogError> {
        let registered = lock(&self.registered);
        let active = Active { config, sinks };
        if let Err(rejected) = self.active.set(active) {
            let winner_has_same_table =
                self.active.get().is_some_and(|current| current.config == rejected.config);
            if winner_has_same_table {
                return Ok(());
            }
            return Err(LogError::InvalidState(
                "a concurrent node creation set a different logging configuration first".to_owned(),
            ));
        }
        let config = self.config();
        for slot in self.slots.iter().take(*registered) {
            let name = slot.name.get().map_or("", String::as_str);
            slot.level.store(config.module_level(name) as u32, Ordering::Release);
        }
        self.lowest_target.store(config.lowest_target_level() as u32, Ordering::Relaxed);
        Ok(())
    }

    fn prepare(
        &self,
        table: &toml::Table,
        file: &Path,
        gate: GateGuard,
    ) -> Result<PreparedConfig, LogError> {
        let config = config::parse(table, file)?;
        if let Some(active) = self.active.get() {
            if active.config == config {
                return Ok(PreparedConfig {
                    config,
                    sinks: Vec::new(),
                    already_active: true,
                    _gate: Some(gate),
                });
            }
            return Err(LogError::InvalidState(format!(
                "{}: [log] the process already has a different logging configuration",
                file.display()
            )));
        }
        let sinks = config
            .sinks
            .iter()
            .map(|sink| {
                file::open(&sink.path, sink.rotation_bytes, sink.rotation_count, sink.level)
                    .map_err(|error| in_context(error, file, &sink.name))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PreparedConfig { config, sinks, already_active: false, _gate: Some(gate) })
    }
}

fn in_context(error: LogError, file: &Path, sink: &str) -> LogError {
    let prefix = format!("{}: [log.sink.{sink}] path: ", file.display());
    match error {
        LogError::InvalidArgument(message) => LogError::InvalidArgument(prefix + &message),
        LogError::InvalidState(message) => LogError::InvalidState(prefix + &message),
        LogError::ResourceLimit => LogError::ResourceLimit,
    }
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

pub(crate) fn engine() -> &'static Engine {
    ENGINE.get_or_init(Engine::new)
}

/// A validated `[log]` table with every enabled file sink opened and locked.
///
/// Pass it to [`commit`](crate::commit), or drop it to discard it.
#[derive(Debug)]
pub struct PreparedConfig {
    config: Config,
    sinks: Vec<LogFile>,
    already_active: bool,
    /// Held until this value is committed or dropped.
    _gate: Option<GateGuard>,
}

/// Validate `table`, the `log` table of `parameter_file`, and open and lock its file
/// sinks. A relative sink path resolves against the directory of `parameter_file`. An
/// equal active configuration gives `Ok` and opens nothing.
///
/// # Errors
///
/// `InvalidArgument` for an invalid table or a file that cannot be opened.
/// `InvalidState` when another holder has locked a sink file, or when the
/// process has a different configuration.
pub fn prepare(table: &toml::Table, parameter_file: &Path) -> Result<PreparedConfig, LogError> {
    crate::facade::install();
    let engine = engine();
    let gate = engine.prepare_gate.enter();
    engine.prepare(table, parameter_file, gate).inspect_err(|error| match error {
        LogError::InvalidArgument(message) | LogError::InvalidState(message) => {
            engine.write_console_message(&format!("hypha_log: {message}"));
        }
        LogError::ResourceLimit => {}
    })
}

/// Make a prepared configuration the process configuration until the process ends.
///
/// # Errors
///
/// `InvalidState` when another creation set a different configuration first.
pub fn commit(prepared: PreparedConfig) -> Result<(), LogError> {
    crate::facade::install();
    if prepared.already_active {
        return Ok(());
    }
    engine().install(prepared.config, prepared.sinks)
}
