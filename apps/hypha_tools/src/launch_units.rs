//! Launch units file: format, validation, and dependency order.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Restart policy for a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestartPolicy {
    /// Never restart (default).
    #[default]
    Never,
    /// Restart only when the process exits with a non-zero code or signal.
    OnFailure,
    /// Always restart when the process exits.
    Always,
}

impl RestartPolicy {
    /// Parse from the TOML launch schema.
    #[must_use]
    pub fn from_str_policy(s: &str) -> Option<Self> {
        match s {
            "no" => Some(Self::Never),
            "on-failure" => Some(Self::OnFailure),
            "always" => Some(Self::Always),
            _ => None,
        }
    }
}

/// Configuration of a single launch unit.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitConfig {
    /// Unique name of the unit.
    pub name: String,
    /// Executable binary path.
    pub exec: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Optional working directory.
    pub working_directory: String,
    /// Restart policy.
    pub restart: RestartPolicy,
    /// Delay in seconds before restart.
    pub restart_delay_seconds: f64,
    /// Names of units that must be started before this unit.
    pub after: Vec<String>,
}

/// Scrollback lines per unit window when `[session]` sets no `historyLimit`.
pub const DEFAULT_HISTORY_LIMIT: u32 = 10_000;
/// Smallest `historyLimit` the units file accepts.
pub const MIN_HISTORY_LIMIT: u32 = 100;
/// Largest `historyLimit` the units file accepts.
pub const MAX_HISTORY_LIMIT: u32 = 1_000_000;

/// Parsed and dependency-ordered units.
#[derive(Debug, Clone)]
pub struct ParsedUnits {
    /// Units in the TOML file's own order.
    pub units: Vec<UnitConfig>,
    /// Indices into `units` in dependency-topological start order.
    pub start_order: Vec<usize>,
    /// Scrollback lines of each unit window.
    pub history_limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLaunchConfig {
    units: Vec<RawUnitConfig>,
    session: Option<RawSessionConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSessionConfig {
    #[serde(rename = "historyLimit")]
    history_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUnitConfig {
    name: Option<String>,
    exec: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    #[serde(rename = "workingDirectory")]
    working_directory: Option<String>,
    restart: Option<String>,
    #[serde(rename = "restartDelaySeconds")]
    restart_delay_seconds: Option<f64>,
    after: Option<Vec<String>>,
}

/// Returns true when `name` holds only ASCII letters, digits, `_` and `-`.
///
/// Unit and session names are tmux targets, and tmux reads `.` and `:` in a
/// target as separators.
#[must_use]
pub fn is_valid_launch_name(name: &str) -> bool {
    let has_characters = !name.is_empty();
    has_characters && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn history_limit_from(session: Option<RawSessionConfig>) -> Result<u32, String> {
    let Some(requested) = session.and_then(|table| table.history_limit) else {
        return Ok(DEFAULT_HISTORY_LIMIT);
    };
    u32::try_from(requested)
        .ok()
        .filter(|limit| (MIN_HISTORY_LIMIT..=MAX_HISTORY_LIMIT).contains(limit))
        .ok_or_else(|| {
            format!(
                "session historyLimit {requested} is not in {MIN_HISTORY_LIMIT}..={MAX_HISTORY_LIMIT}"
            )
        })
}

/// Parses and validates a launch configuration TOML file.
///
/// # Errors
///
/// Returns an error when the file cannot be read, parsed, or fails validation.
pub fn parse_units_file<P: AsRef<Path>>(path: P) -> Result<ParsedUnits, String> {
    let path_ref = path.as_ref();
    let content = fs::read_to_string(path_ref)
        .map_err(|e| format!("cannot read '{}': {e}", path_ref.display()))?;

    let raw_config: RawLaunchConfig = toml::from_str(&content)
        .map_err(|e| format!("cannot parse '{}': {e}", path_ref.display()))?;

    let RawLaunchConfig { units: raw_units, session } = raw_config;
    let history_limit = history_limit_from(session)?;

    let mut units = Vec::new();
    let mut seen_names = HashMap::new();

    for (idx, raw_unit) in raw_units.into_iter().enumerate() {
        let Some(name) = raw_unit.name else {
            return Err("a unit is missing a 'name'".to_string());
        };
        if !is_valid_launch_name(&name) {
            return Err(format!(
                "unit name '{name}' must be non-empty and hold only ASCII letters, digits, '_' and '-'"
            ));
        }
        if seen_names.contains_key(&name) {
            return Err(format!("duplicate unit name '{name}'"));
        }
        seen_names.insert(name.clone(), idx);

        let Some(exec) = raw_unit.exec else {
            return Err(format!("unit '{name}' is missing 'exec'"));
        };
        if exec.trim().is_empty() {
            return Err(format!("unit '{name}' is missing 'exec'"));
        }

        let restart = if let Some(r_str) = raw_unit.restart {
            match RestartPolicy::from_str_policy(&r_str) {
                Some(p) => p,
                None => {
                    return Err(format!(
                        "unit '{name}': restart '{r_str}' is not one of no, on-failure, always"
                    ));
                }
            }
        } else {
            RestartPolicy::Never
        };

        let restart_delay_seconds = raw_unit.restart_delay_seconds.unwrap_or(1.0);
        let delay_is_valid = restart_delay_seconds.is_finite() && restart_delay_seconds >= 0.0;
        if !delay_is_valid {
            return Err(format!(
                "unit '{name}': restartDelaySeconds {restart_delay_seconds} must be a finite number of 0 or more"
            ));
        }

        units.push(UnitConfig {
            name,
            exec,
            args: raw_unit.args.unwrap_or_default(),
            env: raw_unit.env.unwrap_or_default(),
            working_directory: raw_unit.working_directory.unwrap_or_default(),
            restart,
            restart_delay_seconds,
            after: raw_unit.after.unwrap_or_default(),
        });
    }

    if units.is_empty() {
        return Err(format!("'{}' declares no units", path_ref.display()));
    }

    let start_order = order_by_dependencies(&units)?;
    Ok(ParsedUnits { units, start_order, history_limit })
}

/// Computes topological dependency ordering for unit start.
///
/// # Errors
///
/// Returns an error when a dependency is unknown or the dependency graph contains a cycle.
pub fn order_by_dependencies(units: &[UnitConfig]) -> Result<Vec<usize>, String> {
    let mut index_by_name = HashMap::new();
    for (i, u) in units.iter().enumerate() {
        index_by_name.insert(u.name.as_str(), i);
    }

    for unit in units {
        for after_name in &unit.after {
            if !index_by_name.contains_key(after_name.as_str()) {
                return Err(format!(
                    "unit '{}' has after: '{}', which is not a unit in this file",
                    unit.name, after_name
                ));
            }
        }
    }

    let mut placed = vec![false; units.len()];
    let mut order = Vec::with_capacity(units.len());

    while order.len() < units.len() {
        let mut progressed = false;
        for (i, unit) in units.iter().enumerate() {
            if placed.get(i).copied().unwrap_or(false) {
                continue;
            }
            let mut ready = true;
            for after_name in &unit.after {
                let Some(dep_idx) = index_by_name.get(after_name.as_str()).copied() else {
                    return Err(format!(
                        "unit '{}' has after: '{}', which is not a unit in this file",
                        unit.name, after_name
                    ));
                };
                if !placed.get(dep_idx).copied().unwrap_or(false) {
                    ready = false;
                    break;
                }
            }
            if ready {
                if let Some(p) = placed.get_mut(i) {
                    *p = true;
                }
                order.push(i);
                progressed = true;
            }
        }
        if !progressed {
            let mut cycle_units = Vec::new();
            for (i, unit) in units.iter().enumerate() {
                if !placed.get(i).copied().unwrap_or(false) {
                    cycle_units.push(unit.name.clone());
                }
            }
            return Err(format!(
                "cycle in 'after' dependencies -- units still unordered: {}",
                cycle_units.join(" ")
            ));
        }
    }

    Ok(order)
}
