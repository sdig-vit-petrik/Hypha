//! Validation of the `[log]` table.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::LogError;
use crate::level::Level;

const MIB: u64 = 1024 * 1024;
const LOG_KEYS: &[&str] = &["level", "console", "console_level", "sink", "module"];
const SINK_KEYS: &[&str] = &["enabled", "path", "level", "rotation_mb", "rotation_count"];
const MODULE_KEYS: &[&str] = &["level", "enabled"];

/// One enabled file sink with its resolved absolute path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SinkConfig {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) level: Level,
    pub(crate) rotation_bytes: u64,
    pub(crate) rotation_count: u32,
}

/// The validated, resolved configuration. A disabled sink is not part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) level: Level,
    pub(crate) console: bool,
    pub(crate) console_level: Level,
    pub(crate) sinks: Vec<SinkConfig>,
    pub(crate) modules: BTreeMap<String, Level>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            level: Level::Debug,
            console: true,
            console_level: Level::Trace,
            sinks: Vec::new(),
            modules: BTreeMap::new(),
        }
    }
}

impl Config {
    /// The configured level of a module.
    pub(crate) fn module_level(&self, name: &str) -> Level {
        self.modules.get(name).copied().unwrap_or(self.level)
    }

    /// True when the `[log]` table has a `[log.module.<name>]` entry for `name`.
    pub(crate) fn sets_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// The lowest level any target writes. `Off` when there is no target.
    pub(crate) fn lowest_target_level(&self) -> Level {
        let console = self.console.then_some(self.console_level);
        let sinks = self.sinks.iter().map(|sink| sink.level);
        console.into_iter().chain(sinks).min().unwrap_or(Level::Off)
    }
}

/// True when `name` follows the module name rule: 1 to 64 bytes from
/// `A-Z`, `a-z`, `0-9`, `_` and `/`.
pub(crate) fn valid_module_name(name: &str) -> bool {
    let length_ok = (1..=64).contains(&name.len());
    length_ok
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'/')
}

/// Names the file, the table and the key of a refused value.
struct Source<'a> {
    file: &'a Path,
}

impl Source<'_> {
    fn refuse(&self, table: &str, key: &str, reason: &str) -> LogError {
        LogError::InvalidArgument(format!("{}: [{table}] {key}: {reason}", self.file.display()))
    }

    fn check_keys(
        &self,
        table_name: &str,
        table: &Table,
        allowed: &[&str],
    ) -> Result<(), LogError> {
        match table.keys().find(|key| !allowed.contains(&key.as_str())) {
            Some(key) => Err(self.refuse(table_name, key, "unknown key")),
            None => Ok(()),
        }
    }

    fn level(&self, table_name: &str, table: &Table, key: &str) -> Result<Option<Level>, LogError> {
        let Some(value) = table.get(key) else { return Ok(None) };
        let Value::String(name) = value else {
            return Err(self.refuse(table_name, key, "must be a string"));
        };
        Level::from_name(name)
            .map(Some)
            .ok_or_else(|| self.refuse(table_name, key, &format!("unknown level {name:?}")))
    }

    fn boolean(
        &self,
        table_name: &str,
        table: &Table,
        key: &str,
    ) -> Result<Option<bool>, LogError> {
        match table.get(key) {
            None => Ok(None),
            Some(Value::Boolean(flag)) => Ok(Some(*flag)),
            Some(_) => Err(self.refuse(table_name, key, "must be a boolean")),
        }
    }

    fn ranged(
        &self,
        table_name: &str,
        table: &Table,
        key: &str,
        range: std::ops::RangeInclusive<u32>,
        default: u32,
    ) -> Result<u32, LogError> {
        let Some(value) = table.get(key) else { return Ok(default) };
        let Value::Integer(number) = value else {
            return Err(self.refuse(table_name, key, "must be an integer"));
        };
        u32::try_from(*number).ok().filter(|number| range.contains(number)).ok_or_else(|| {
            let (low, high) = (range.start(), range.end());
            self.refuse(table_name, key, &format!("must be from {low} to {high}"))
        })
    }

    fn subtables<'t>(
        &self,
        table: &'t Table,
        key: &str,
    ) -> Result<Vec<(&'t String, &'t Table)>, LogError> {
        let Some(value) = table.get(key) else { return Ok(Vec::new()) };
        let Value::Table(entries) = value else {
            return Err(self.refuse("log", key, "must be a table"));
        };
        let table_name = format!("log.{key}");
        entries
            .iter()
            .map(|(name, entry)| match entry {
                Value::Table(inner) => Ok((name, inner)),
                _ => Err(self.refuse(&table_name, name, "must be a table")),
            })
            .collect()
    }

    fn sink(&self, name: &str, table: &Table, base: &Path) -> Result<Option<SinkConfig>, LogError> {
        let table_name = format!("log.sink.{name}");
        self.check_keys(&table_name, table, SINK_KEYS)?;
        let enabled = self.boolean(&table_name, table, "enabled")?.unwrap_or(true);
        let level = self.level(&table_name, table, "level")?.unwrap_or(Level::Info);
        let megabytes = self.ranged(&table_name, table, "rotation_mb", 1..=1024, 5)?;
        let rotation_count = self.ranged(&table_name, table, "rotation_count", 1..=100, 3)?;
        let path = match table.get("path") {
            None => return Err(self.refuse(&table_name, "path", "is required")),
            Some(Value::String(text)) if text.is_empty() => {
                return Err(self.refuse(&table_name, "path", "must not be empty"));
            }
            Some(Value::String(text)) => text,
            Some(_) => return Err(self.refuse(&table_name, "path", "must be a string")),
        };
        if !enabled {
            return Ok(None);
        }
        let path = std::path::absolute(base.join(path)).map_err(|error| {
            self.refuse(&table_name, "path", &format!("cannot make the path absolute: {error}"))
        })?;
        let rotation_bytes = u64::from(megabytes).saturating_mul(MIB);
        Ok(Some(SinkConfig { name: name.to_string(), path, level, rotation_bytes, rotation_count }))
    }
}

/// Validate `table`, the value of the `log` key of `file`.
pub(crate) fn parse(table: &Table, file: &Path) -> Result<Config, LogError> {
    let source = Source { file };
    source.check_keys("log", table, LOG_KEYS)?;
    let mut config = Config::default();
    config.level = source.level("log", table, "level")?.unwrap_or(config.level);
    config.console = source.boolean("log", table, "console")?.unwrap_or(config.console);
    config.console_level =
        source.level("log", table, "console_level")?.unwrap_or(config.console_level);

    let base = file.parent().unwrap_or_else(|| Path::new(""));
    let mut owners: HashMap<PathBuf, String> = HashMap::new();
    for (name, entry) in source.subtables(table, "sink")? {
        let Some(sink) = source.sink(name, entry, base)? else { continue };
        if let Some(first) = owners.insert(sink.path.clone(), name.clone()) {
            let reason = format!("{} is also the path of sink {first}", sink.path.display());
            return Err(source.refuse(&format!("log.sink.{name}"), "path", &reason));
        }
        config.sinks.push(sink);
    }
    for (name, entry) in source.subtables(table, "module")? {
        if !valid_module_name(name) {
            return Err(source.refuse("log.module", name, "invalid module name"));
        }
        let table_name = format!("log.module.{name}");
        source.check_keys(&table_name, entry, MODULE_KEYS)?;
        let enabled = source.boolean(&table_name, entry, "enabled")?.unwrap_or(true);
        let level = source.level(&table_name, entry, "level")?.unwrap_or(config.level);
        config.modules.insert(name.clone(), if enabled { level } else { Level::Off });
    }
    Ok(config)
}
