//! Standalone parameter configuration handle and unread-key queries.

use std::path::Path;
use std::sync::Arc;

use hypha_core::introspection::ConfigRow;
use hypha_core::parameters::{ParamValue, Parameters};

use crate::status::Error;

/// A loaded parameter configuration, independent of any bus session. Opaque to the caller.
#[derive(Clone)]
pub struct HyphaConfig {
    pub(crate) parameters: Arc<Parameters>,
    pub(crate) parameter_file: Option<std::path::PathBuf>,
    pub(crate) introspection_rows: Arc<std::sync::Mutex<Vec<ConfigRow>>>,
}

impl std::ops::Deref for HyphaConfig {
    type Target = Parameters;

    fn deref(&self) -> &Self::Target {
        &self.parameters
    }
}

impl HyphaConfig {
    pub(crate) fn record_introspection_row(&self, row: ConfigRow) {
        let mut rows = crate::locked(&self.introspection_rows);
        if let Some(known) = rows.iter_mut().find(|known| known.key == row.key) {
            *known = row;
        } else {
            rows.push(row);
        }
    }

    pub(crate) fn introspection_rows(&self) -> Vec<ConfigRow> {
        self.introspection_rows.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// Loads `parameter_file`, or makes an empty configuration for `None`, without
/// joining the bus.
///
/// # Errors
/// `InvalidArgument` when the file cannot be read or parsed.
pub fn config_create(parameter_file: Option<&str>) -> Result<HyphaConfig, Error> {
    // A configuration is loaded before any node exists, so the log line needs the facade.
    hypha_log::install();
    load_config(parameter_file)
}

/// A new configuration holding the parameters of `parameter_file`, or none when it is
/// absent. A refused file is logged and gives `InvalidArgument`.
pub(crate) fn load_config(parameter_file: Option<&str>) -> Result<HyphaConfig, Error> {
    let parameter_file = parameter_file.map(std::path::PathBuf::from);
    let parameters = match &parameter_file {
        None => Parameters::new(),
        Some(path) => Parameters::from_toml_file(Path::new(path)).map_err(|cause| {
            log::error!(target: "hypha", "hypha: {cause}");
            Error::InvalidArgument
        })?,
    };
    Ok(HyphaConfig {
        parameters: Arc::new(parameters),
        parameter_file,
        introspection_rows: Arc::default(),
    })
}

/// The unread keys of `config` under `prefix` (every key for an empty prefix),
/// joined by `'\n'`. The framework key `node.name` is excluded.
#[must_use]
pub fn config_unread_keys(config: &HyphaConfig, prefix: &str) -> String {
    config.parameters.unread_keys_under(prefix).join("\n")
}

/// The file name that the text parameter `key` holds, resolved against the directory
/// of the file that wrote `key`. Marks `key` read.
///
/// # Errors
/// `InvalidArgument` when `key` names no text value or an empty one, or the result
/// is not UTF-8.
pub fn config_resolve_key(config: &HyphaConfig, key: &str) -> Result<String, Error> {
    let parameters = &*config.parameters;
    let Some(ParamValue::Text(name)) = crate::parameter_parse::scalar_at_key(parameters, key)
    else {
        return Err(Error::InvalidArgument);
    };
    if name.is_empty() {
        return Err(Error::InvalidArgument);
    }
    let resolved = match parameters.origin_of(key) {
        Some(directory) => directory.join(name),
        None => Path::new(name).to_path_buf(),
    };
    resolved.to_str().map(str::to_owned).ok_or(Error::InvalidArgument)
}

/// The configuration of `node`; it lives as long as `node`.
#[must_use]
pub fn node_config(node: &crate::node::HyphaNode) -> &HyphaConfig {
    &node.parameters
}
