//! The two documents of `hypha intercept`: the config file that holds the desired
//! interceptions, and the state document the backend publishes on
//! [`state_topic`].

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::msg_patch::{Override, ScalarValue, parse_value};

/// The config file: what should be intercepted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterceptConfig {
    /// Bumped by every writer. The backend echoes it once applied.
    #[serde(default)]
    pub revision: u64,
    /// The interceptions, by unique name.
    #[serde(default)]
    pub interception: Vec<InterceptionSpec>,
}

/// One interception: read `input`, rewrite fields, publish on `output`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterceptionSpec {
    /// Unique, `[a-z0-9_]+`.
    pub name: String,
    /// The topic read.
    pub input: String,
    /// The topic written, under the input's type identity.
    pub output: String,
    /// `false` forwards every frame unchanged.
    #[serde(default = "armed_by_default")]
    pub armed: bool,
    /// Overrides by field path.
    #[serde(default)]
    pub overrides: BTreeMap<String, OverrideSpec>,
}

fn armed_by_default() -> bool {
    true
}

/// One override as the file stores it. Values stay text: they are parsed against
/// the field type when they are applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OverrideSpec {
    /// `{ value = "..." }`.
    Value {
        /// The new value.
        value: String,
    },
    /// `{ set = "0x2", clear = "0x8" }`; either defaults to `"0"`.
    Bits {
        /// Bits forced to 1.
        #[serde(default = "no_bits")]
        set: String,
        /// Bits forced to 0.
        #[serde(default = "no_bits")]
        clear: String,
    },
}

fn no_bits() -> String {
    "0".to_owned()
}

impl OverrideSpec {
    /// The override this entry describes.
    ///
    /// # Errors
    /// Returns the reason when a bit mask is not a 64-bit unsigned number.
    pub fn to_override(&self) -> Result<Override, String> {
        match self {
            Self::Value { value } => Ok(Override::Value(value.clone())),
            Self::Bits { set, clear } => {
                Ok(Override::Bits { set: mask(set)?, clear: mask(clear)? })
            }
        }
    }
}

fn mask(text: &str) -> Result<u64, String> {
    match parse_value("uint64", text)? {
        ScalarValue::UInt(bits) => Ok(bits),
        other => Err(format!("{other:?} is not a bit mask")),
    }
}

/// True for a name made of `[a-z0-9_]`, at least one character long.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Parses and checks a config: names valid and unique, no interception reading its
/// own output, no two writing the same output.
///
/// # Errors
/// Returns the reason, naming the interception, when the text is not a valid config.
pub fn parse_config(text: &str) -> Result<InterceptConfig, String> {
    let config: InterceptConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut names = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for spec in &config.interception {
        let name = &spec.name;
        if !valid_name(name) {
            return Err(format!("interception '{name}': a name is [a-z0-9_]+"));
        }
        if !names.insert(name.as_str()) {
            return Err(format!("interception '{name}' is defined twice"));
        }
        if spec.input.is_empty() || spec.output.is_empty() {
            return Err(format!("interception '{name}' needs an input and an output topic"));
        }
        if spec.input == spec.output {
            return Err(format!("interception '{name}' reads its own output '{}'", spec.input));
        }
        if !outputs.insert(spec.output.as_str()) {
            return Err(format!(
                "interception '{name}' writes '{}', which another interception writes",
                spec.output
            ));
        }
    }
    Ok(config)
}

/// Reads the config at `path`. A missing file is an empty config at revision 0.
///
/// # Errors
/// Returns the reason when the file cannot be read or is not a valid config.
pub fn load_config(path: &Path) -> Result<InterceptConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_config(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(InterceptConfig::default())
        }
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Writes `config` to `path` so that a reader sees the old file or the new one,
/// never a part: a temporary file beside it, synced, then renamed over it.
///
/// # Errors
/// Returns the reason when the file cannot be written.
pub fn write_config_atomic(path: &Path, config: &InterceptConfig) -> Result<(), String> {
    let text = toml::to_string(config).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()));
    let written = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    let renamed = written.and_then(|()| std::fs::rename(&temporary, path));
    if let Err(error) = renamed {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    Ok(())
}

/// The state document a backend publishes, latched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendState {
    /// The backend name.
    #[serde(default)]
    pub backend: String,
    /// The absolute path of the config file it reads.
    #[serde(default)]
    pub config_path: String,
    /// The revision of the config it applies.
    #[serde(default)]
    pub applied_revision: u64,
    /// Non-empty when the last load failed; the previous config stays applied.
    #[serde(default)]
    pub config_error: String,
    /// One entry per applied interception.
    #[serde(default)]
    pub interception: Vec<InterceptionState>,
}

/// The state and counters of one interception.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterceptionState {
    /// The interception name.
    pub name: String,
    /// Where the relay is.
    #[serde(default)]
    pub state: RelayState,
    /// The type of the input, once resolved.
    #[serde(default)]
    pub data_type: String,
    /// Frames read.
    #[serde(default)]
    pub received: u64,
    /// Frames with at least one override written.
    #[serde(default)]
    pub modified: u64,
    /// Frames published.
    #[serde(default)]
    pub published: u64,
    /// Frames with an override that could not be written.
    #[serde(default)]
    pub patch_failed: u64,
    /// Frames read before the schema resolved. They cannot be republished.
    #[serde(default)]
    pub unresolved_dropped: u64,
    /// The newest failure, empty when there was none.
    #[serde(default)]
    pub last_error: String,
}

/// Where one relay is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    /// Waiting for the input's schema.
    #[default]
    Resolving,
    /// Forwarding.
    Live,
    /// The input's schema changed; nothing is forwarded until the interception is
    /// rebuilt.
    SchemaChanged,
    /// The interception cannot run; `last_error` says why.
    Error,
}

/// The topic of backend `backend`'s state document.
#[must_use]
pub fn state_topic(backend: &str) -> String {
    format!("/hypha/intercept/{backend}/state")
}
