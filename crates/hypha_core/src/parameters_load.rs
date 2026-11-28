//! TOML file composition, flattening, and environment substitution for [`Parameters`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::parameters::{
    ListElement, ParamValue, ParameterLoadError, ParameterOverride, Parameters, key_is_under,
};

const LOG_TABLE: &str = "log";
const INCLUDE_KEY: &str = "include";
/// The top-level key that lists the keys a file sets on purpose and no code reads.
const UNUSED_KEY: &str = "unused";
const SINK_TABLE: &str = "sink";
const SINK_PATH_KEY: &str = "path";

/// The file that wrote each scalar and list, by dotted name, `[log]` keys included.
type Origins = BTreeMap<String, PathBuf>;
const NESTED_LIST_REASON: &str = "a list cannot hold a list; put it in a field of a table element";
/// How many include levels a file may sit below the root file.
const MAX_INCLUDE_DEPTH: usize = 16;
/// How many included files one composition reads, a file included twice counted twice.
const MAX_INCLUDED_FILES: usize = 256;

pub(crate) fn load_from_toml_file(path: &Path) -> Result<Parameters, ParameterLoadError> {
    let path_text = path.display().to_string();
    let root = std::fs::canonicalize(path).map_err(|error| read_error(&path_text, &error))?;
    let mut sources = Sources::default();
    let table = read_composed(path, &mut vec![root], &mut sources)?;
    from_table(table, &path_text, sources)
}

/// Where the values of a composed file set came from.
#[derive(Default)]
struct Sources {
    origins: Origins,
    overrides: Vec<ParameterOverride>,
    /// The included files read so far, against [`MAX_INCLUDED_FILES`].
    included_files: usize,
    /// Each file's `unused` entries, with the file that listed them, in file order.
    unused: Vec<(String, PathBuf)>,
}

fn from_table(
    mut table: Table,
    path: &str,
    sources: Sources,
) -> Result<Parameters, ParameterLoadError> {
    let log = match table.remove(LOG_TABLE) {
        None => None,
        Some(Value::Table(log)) => Some(log),
        Some(_) => return Err(load_error(path, LOG_TABLE, "the value must be a table")),
    };

    let mut params = Parameters::with_sources(log, sources.origins, sources.overrides);
    flatten_table(&table, "", path, "", &mut params)?;
    refuse_name_conflicts(&params, path)?;
    ignore_listed_keys(&params, &sources.unused)?;
    Ok(params)
}

/// Ignore each `unused` entry. An entry that names no key of the composed set is refused,
/// so the list cannot go stale or hide a typo.
fn ignore_listed_keys(
    params: &Parameters,
    unused: &[(String, PathBuf)],
) -> Result<(), ParameterLoadError> {
    let configured = params.configured_keys_at_load();
    for (entry, file) in unused {
        let is_set = configured.iter().any(|key| key_is_under(key, entry));
        if !is_set {
            let reason = format!("`{entry}` is not set in the configuration");
            return Err(load_error(&file.display().to_string(), UNUSED_KEY, &reason));
        }
        params.ignore_listed(entry, file);
    }
    Ok(())
}

/// Read `path` and merge the files it includes. `stack` holds the canonical paths of
/// the files that lead to `path`, root first. The root file overrides its includes at
/// any depth; any other duplicate is refused.
fn read_composed(
    path: &Path,
    stack: &mut Vec<PathBuf>,
    sources: &mut Sources,
) -> Result<Table, ParameterLoadError> {
    let is_root = stack.len() == 1;
    let path_text = path.display().to_string();
    let source = std::fs::read_to_string(path).map_err(|error| read_error(&path_text, &error))?;
    let mut table = parse_table(&source, &path_text)?;
    // Expanded per file, before the includes, so an error names the file that holds it.
    expand_table(&mut table, &path_text, "")?;
    let listed = take_includes(&mut table, &path_text)?;
    let unused = take_unused(&mut table, &path_text)?;
    let directory = path.parent().unwrap_or(Path::new(""));
    absolutize_sink_paths(&mut table, directory);
    let mut merged = Table::new();
    for include in &listed {
        let included = read_included(include, directory, &path_text, stack, sources)?;
        merge_tables(&mut merged, included, "", &path_text)?;
    }
    if is_root {
        overlay_root(&mut merged, table.clone(), "", path, sources)?;
    } else {
        merge_tables(&mut merged, table.clone(), "", &path_text)?;
    }
    // Recorded after the includes: the value this file writes is the one that stays.
    record_origins(&table, "", path, &mut sources.origins);
    sources.unused.extend(unused.into_iter().map(|entry| (entry, path.to_path_buf())));
    Ok(merged)
}

/// Read one included file. Refuses an unreadable file, a cycle, and a file past the
/// depth or file limit.
fn read_included(
    include: &str,
    directory: &Path,
    owner: &str,
    stack: &mut Vec<PathBuf>,
    sources: &mut Sources,
) -> Result<Table, ParameterLoadError> {
    let target = directory.join(include);
    let canonical = std::fs::canonicalize(&target).map_err(|error| {
        let reason = format!("included file `{}` cannot be read: {error}", target.display());
        load_error(owner, INCLUDE_KEY, &reason)
    })?;
    if stack.contains(&canonical) {
        let reason = format!("include cycle at `{}`", target.display());
        return Err(load_error(owner, INCLUDE_KEY, &reason));
    }
    if stack.len() > MAX_INCLUDE_DEPTH {
        let reason = format!("includes nest deeper than {MAX_INCLUDE_DEPTH} levels");
        return Err(load_error(owner, INCLUDE_KEY, &reason));
    }
    sources.included_files = sources.included_files.saturating_add(1);
    if sources.included_files > MAX_INCLUDED_FILES {
        let reason = format!("includes read more than {MAX_INCLUDED_FILES} files");
        return Err(load_error(owner, INCLUDE_KEY, &reason));
    }
    stack.push(canonical);
    let table = read_composed(&target, stack, sources);
    stack.pop();
    table
}

fn record_origins(table: &Table, prefix: &str, file: &Path, origins: &mut Origins) {
    for (name, value) in table {
        let key = join_parameter_key(prefix, name);
        match value {
            Value::Table(nested) => record_origins(nested, &key, file, origins),
            _ => {
                origins.insert(key, file.to_path_buf());
            }
        }
    }
}

/// Overlay the root file's own `root` table on `merged`, its includes. A root scalar or
/// list replaces the same key and is recorded as an override. A change of kind is refused.
fn overlay_root(
    merged: &mut Table,
    root: Table,
    prefix: &str,
    root_file: &Path,
    sources: &mut Sources,
) -> Result<(), ParameterLoadError> {
    let root_text = root_file.display().to_string();
    for (key, value) in root {
        let full_key = join_parameter_key(prefix, &key);
        let Some(existing) = merged.get_mut(&key) else {
            merged.insert(key, value);
            continue;
        };
        let both_tables = existing.is_table() && value.is_table();
        if both_tables {
            let (Value::Table(existing_table), Value::Table(root_table)) = (existing, value) else {
                return Err(load_error(&root_text, &full_key, "conflicts with an included file"));
            };
            overlay_root(existing_table, root_table, &full_key, root_file, sources)?;
            continue;
        }
        let existing_kind = value_kind(existing);
        let same_kind = existing_kind == value_kind(&value) && existing_kind != ValueKind::Table;
        if !same_kind {
            return Err(load_error(&root_text, &full_key, "conflicts with an included file"));
        }
        let overridden = sources.origins.get(&full_key).cloned().unwrap_or_default();
        sources.overrides.push(ParameterOverride {
            key: full_key,
            winner: root_file.to_path_buf(),
            overridden,
        });
        *existing = value;
    }
    Ok(())
}

/// The shape of a TOML value, for the root overlay.
#[derive(PartialEq, Eq, Clone, Copy)]
enum ValueKind {
    Scalar,
    List,
    Table,
}

fn value_kind(value: &Value) -> ValueKind {
    match value {
        Value::Table(_) => ValueKind::Table,
        Value::Array(_) => ValueKind::List,
        Value::String(_)
        | Value::Integer(_)
        | Value::Float(_)
        | Value::Boolean(_)
        | Value::Datetime(_) => ValueKind::Scalar,
    }
}

/// Make each relative `[log.sink.<name>] path` of a file's own table absolute against
/// `directory`, the directory of that file.
fn absolutize_sink_paths(table: &mut Table, directory: &Path) {
    let Some(Value::Table(log)) = table.get_mut(LOG_TABLE) else {
        return;
    };
    let Some(Value::Table(sinks)) = log.get_mut(SINK_TABLE) else {
        return;
    };
    for (_name, sink) in sinks.iter_mut() {
        let Value::Table(sink) = sink else {
            continue;
        };
        let Some(Value::String(text)) = sink.get_mut(SINK_PATH_KEY) else {
            continue;
        };
        let relative = !text.is_empty() && Path::new(text.as_str()).is_relative();
        if !relative {
            continue;
        }
        let joined = directory.join(text.as_str());
        let Ok(absolute) = std::path::absolute(&joined) else {
            continue;
        };
        let Some(absolute_text) = absolute.to_str() else {
            continue;
        };
        absolute_text.clone_into(text);
    }
}

/// Replace the `${NAME}` references in every string of `table`, at any depth.
/// Errors name the dotted key of the value.
fn expand_table(table: &mut Table, path: &str, prefix: &str) -> Result<(), ParameterLoadError> {
    for (name, value) in table.iter_mut() {
        let key = join_parameter_key(prefix, name);
        let is_include = prefix.is_empty() && name == INCLUDE_KEY;
        if is_include {
            expand_include(value, path)?;
            continue;
        }
        expand_value(value, path, &key)?;
    }
    Ok(())
}

/// Expand one value stored under `key`.
fn expand_value(value: &mut Value, path: &str, key: &str) -> Result<(), ParameterLoadError> {
    match value {
        Value::String(text) => {
            *text = substitute_environment(text, path, key)?;
        }
        Value::Table(nested) => expand_table(nested, path, key)?,
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                let label = format!("{key}[{index}]");
                expand_value(item, path, &label)?;
            }
        }
        Value::Integer(_) | Value::Float(_) | Value::Boolean(_) | Value::Datetime(_) => {}
    }
    Ok(())
}

/// Expand the entries of `include`. Every error names the key `include`.
fn expand_include(value: &mut Value, path: &str) -> Result<(), ParameterLoadError> {
    let Value::Array(items) = value else {
        return Ok(());
    };
    for item in items.iter_mut() {
        if let Value::String(text) = item {
            *text = substitute_environment(text, path, INCLUDE_KEY)?;
        }
    }
    Ok(())
}

/// Remove the `include` key from `table` and return its paths.
fn take_includes(table: &mut Table, path: &str) -> Result<Vec<String>, ParameterLoadError> {
    let Some(value) = table.remove(INCLUDE_KEY) else {
        return Ok(Vec::new());
    };
    let Value::Array(items) = value else {
        return Err(include_error(path));
    };
    items
        .iter()
        .map(|item| item.as_str().map(str::to_string).ok_or_else(|| include_error(path)))
        .collect()
}

/// Remove the `unused` key from `table` and return its entries, each a nonempty key name.
fn take_unused(table: &mut Table, path: &str) -> Result<Vec<String>, ParameterLoadError> {
    let Some(value) = table.remove(UNUSED_KEY) else {
        return Ok(Vec::new());
    };
    let refused = || load_error(path, UNUSED_KEY, "the value must be an array of key names");
    let Value::Array(items) = value else {
        return Err(refused());
    };
    items
        .iter()
        .map(|item| match item.as_str() {
            Some(name) if !name.is_empty() => Ok(name.to_string()),
            _ => Err(refused()),
        })
        .collect()
}

/// Merge `later` into `earlier`. Tables merge key by key; any other duplicate is refused.
fn merge_tables(
    earlier: &mut Table,
    later: Table,
    prefix: &str,
    path: &str,
) -> Result<(), ParameterLoadError> {
    for (key, value) in later {
        let full_key = join_parameter_key(prefix, &key);
        let Some(existing) = earlier.get_mut(&key) else {
            earlier.insert(key, value);
            continue;
        };
        let (Value::Table(existing_table), Value::Table(incoming_table)) = (existing, value) else {
            return Err(load_error(path, &full_key, "conflicts with an included file"));
        };
        merge_tables(existing_table, incoming_table, &full_key, path)?;
    }
    Ok(())
}

fn parse_table(source: &str, path: &str) -> Result<Table, ParameterLoadError> {
    toml::from_str::<Table>(source).map_err(|error| load_error(path, "", &error.to_string()))
}

fn read_error(path: &str, error: &std::io::Error) -> ParameterLoadError {
    load_error(path, "", &error.to_string())
}

fn include_error(path: &str) -> ParameterLoadError {
    load_error(path, INCLUDE_KEY, "the value must be an array of strings")
}

/// Store every value of `table` in `params` under dotted names from `prefix`.
fn flatten_table(
    table: &Table,
    prefix: &str,
    path: &str,
    label: &str,
    params: &mut Parameters,
) -> Result<(), ParameterLoadError> {
    for (name, value) in table {
        let key = join_parameter_key(prefix, name);
        match value {
            Value::Table(nested) => flatten_table(nested, &key, path, label, params)?,
            Value::Array(items) => read_list(items, &key, path, label, params)?,
            _ => {
                let full_key = join_parameter_key(label, &key);
                let value = scalar_value(value, path, &full_key)?;
                if params.insert_scalar(key, value).is_some() {
                    return Err(load_error(path, &full_key, "conflicts with another key"));
                }
            }
        }
    }
    Ok(())
}

/// Store the array `items` as the list `key`, of the table element `label` when it is not
/// empty. A list holds one kind of element: all scalars or all tables, never lists.
fn read_list(
    items: &[Value],
    key: &str,
    path: &str,
    label: &str,
    params: &mut Parameters,
) -> Result<(), ParameterLoadError> {
    let full_key = join_parameter_key(label, key);
    let tables = items.iter().filter(|item| item.is_table()).count();
    let mixed = tables != 0 && tables != items.len();
    if mixed {
        return Err(load_error(path, &full_key, "a list cannot mix scalars and tables"));
    }
    let elements = items
        .iter()
        .enumerate()
        .map(|(index, item)| read_element(item, &full_key, index, path))
        .collect::<Result<_, _>>()?;
    if params.insert_list(key.to_string(), elements).is_some() {
        return Err(load_error(path, &full_key, "conflicts with another key"));
    }
    Ok(())
}

/// Read element `index` of the list `key` (its full name). A table element may hold lists.
fn read_element(
    item: &Value,
    key: &str,
    index: usize,
    path: &str,
) -> Result<ListElement, ParameterLoadError> {
    let label = format!("{key}[{index}]");
    match item {
        Value::Table(table) => {
            let mut params = Parameters::default();
            flatten_table(table, "", path, &label, &mut params)?;
            Ok(ListElement::Table(params))
        }
        Value::Array(_) => Err(load_error(path, &label, NESTED_LIST_REASON)),
        _ => scalar_value(item, path, &label).map(ListElement::Scalar),
    }
}

/// The scalar a TOML value holds.
fn scalar_value(value: &Value, path: &str, key: &str) -> Result<ParamValue, ParameterLoadError> {
    match value {
        Value::String(text) => Ok(ParamValue::Text(text.clone())),
        Value::Integer(number) => Ok(ParamValue::Int(*number)),
        Value::Float(number) => Ok(ParamValue::Float(*number)),
        Value::Boolean(flag) => Ok(ParamValue::Bool(*flag)),
        Value::Datetime(_) => Err(load_error(path, key, "date-time values are not supported")),
        Value::Table(_) | Value::Array(_) => Err(load_error(path, key, "not a scalar")),
    }
}

/// Refuse a list whose name equals, or dot-nests with, a scalar name, in `params` and in
/// each of its table elements.
fn refuse_name_conflicts(params: &Parameters, path: &str) -> Result<(), ParameterLoadError> {
    refuse_name_conflicts_in(params, path, "")
}

fn refuse_name_conflicts_in(
    params: &Parameters,
    path: &str,
    label: &str,
) -> Result<(), ParameterLoadError> {
    for list in params.list_names() {
        let clash = params.names().into_iter().find(|scalar| names_overlap(list, scalar));
        if let Some(scalar) = clash {
            let reason = format!("conflicts with scalar `{scalar}`");
            return Err(load_error(path, &join_parameter_key(label, list), &reason));
        }
    }
    for (element_label, element) in params.table_elements() {
        refuse_name_conflicts_in(element, path, &join_parameter_key(label, &element_label))?;
    }
    Ok(())
}

/// True when two names are equal, or one is a dotted child of the other.
fn names_overlap(first: &str, second: &str) -> bool {
    let (short, long) = if first.len() <= second.len() { (first, second) } else { (second, first) };
    long == short || long.strip_prefix(short).is_some_and(|rest| rest.starts_with('.'))
}

fn substitute_environment(text: &str, path: &str, key: &str) -> Result<String, ParameterLoadError> {
    let mut expanded = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some((literal, reference)) = remaining.split_once("${") {
        expanded.push_str(literal);
        let Some((name, after_reference)) = reference.split_once('}') else {
            return Err(load_error(path, key, "unterminated environment reference"));
        };
        if !valid_environment_name(name) {
            return Err(load_error(path, key, "invalid environment variable name"));
        }
        let value = match std::env::var(name) {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => {
                let reason = format!("environment variable `{name}` is unset");
                return Err(load_error(path, key, &reason));
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                let reason = format!("environment variable `{name}` is not readable text");
                return Err(load_error(path, key, &reason));
            }
        };
        if value.is_empty() {
            let reason = format!("environment variable `{name}` is empty");
            return Err(load_error(path, key, &reason));
        }
        expanded.push_str(&value);
        remaining = after_reference;
    }
    expanded.push_str(remaining);
    Ok(expanded)
}

fn valid_environment_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn join_parameter_key(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        return name.to_string();
    }
    format!("{prefix}.{name}")
}

fn load_error(path: &str, key: &str, reason: &str) -> ParameterLoadError {
    ParameterLoadError { path: path.to_string(), key: key.to_string(), reason: reason.to_string() }
}
