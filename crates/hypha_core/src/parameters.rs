//! Configuration a node reads at start-up.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use toml::Table;

use crate::parameters_load::load_from_toml_file;

const RESERVED_SCALARS: [&str; 1] = ["node.name"];

#[derive(Default)]
struct ReadSet(Mutex<BTreeSet<String>>);

impl ReadSet {
    fn insert(&self, name: &str) {
        crate::locked(&self.0).insert(name.to_string());
    }

    fn contains(&self, name: &str) -> bool {
        crate::locked(&self.0).contains(name)
    }
}

impl Clone for ReadSet {
    fn clone(&self) -> Self {
        let cloned = crate::locked(&self.0).clone();
        Self(Mutex::new(cloned))
    }
}

impl PartialEq for ReadSet {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ReadSet {}

/// A key or table that is set on purpose and not read, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredKey {
    /// The dotted key, a table prefix, or a list entry (`sensors[0].rate`).
    pub key: String,
    /// Why nothing reads it.
    pub reason: String,
    /// True when a file's `unused` list named it; false when code ignored it.
    pub listed_in_file: bool,
}

/// The ignored keys, in the order they were first ignored. Shared like [`ReadSet`].
#[derive(Default)]
struct IgnoreSet(Mutex<Vec<IgnoredKey>>);

impl IgnoreSet {
    /// Adds `entry` unless its key is already ignored; true when it was added.
    fn insert(&self, entry: IgnoredKey) -> bool {
        let mut entries = crate::locked(&self.0);
        if entries.iter().any(|known| known.key == entry.key) {
            return false;
        }
        entries.push(entry);
        true
    }

    fn snapshot(&self) -> Vec<IgnoredKey> {
        crate::locked(&self.0).clone()
    }

    fn covers(&self, key: &str) -> bool {
        let entries = crate::locked(&self.0);
        entries.iter().any(|entry| key_is_under(key, &entry.key))
    }
}

impl Clone for IgnoreSet {
    fn clone(&self) -> Self {
        Self(Mutex::new(self.snapshot()))
    }
}

impl PartialEq for IgnoreSet {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl fmt::Debug for IgnoreSet {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.snapshot(), out)
    }
}

/// `list[index]` or `list[index].inner` as the list, the index and what follows the dot.
fn split_element_name(name: &str) -> Option<(&str, usize, &str)> {
    let (list, after_bracket) = name.split_once('[')?;
    let (index_text, after_index) = after_bracket.split_once(']')?;
    let index_is_digits =
        !index_text.is_empty() && index_text.bytes().all(|byte| byte.is_ascii_digit());
    if !index_is_digits {
        return None;
    }
    let inner = if after_index.is_empty() { "" } else { after_index.strip_prefix('.')? };
    Some((list, index_text.parse().ok()?, inner))
}

/// True when `key` is `prefix` or a dotted or indexed name under it.
pub(crate) fn key_is_under(key: &str, prefix: &str) -> bool {
    key == prefix
        || key
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

impl fmt::Debug for ReadSet {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let set = crate::locked(&self.0);
        fmt::Debug::fmt(&*set, out)
    }
}

/// Why a TOML parameter file could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterLoadError {
    /// The configured file path or source label.
    pub path: String,
    /// The parameter key that caused the error, when available.
    pub key: String,
    /// The reason the file could not be loaded.
    pub reason: String,
}

impl fmt::Display for ParameterLoadError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "could not load TOML configuration `{}`", self.path)?;
        if !self.key.is_empty() {
            write!(out, " at `{}`", self.key)?;
        }
        write!(out, ": {}", self.reason)
    }
}

impl std::error::Error for ParameterLoadError {}

/// A parameter value, with the type the TOML file gave it.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    /// `true` or `false`.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A number with a fraction.
    Float(f64),
    /// Text, with `${NAME}` references already replaced.
    Text(String),
}

impl fmt::Display for ParamValue {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(out, "{value}"),
            Self::Int(value) => write!(out, "{value}"),
            // The Debug form keeps the decimal point and the exponent, as TOML does.
            Self::Float(value) if value.is_nan() => write!(out, "nan"),
            Self::Float(value) => write!(out, "{value:?}"),
            Self::Text(value) => write!(out, "{value:?}"),
        }
    }
}

/// A key of an included file that the root file replaced with its own value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterOverride {
    /// The dotted key; a list replaced whole names the list.
    pub key: String,
    /// The root file, whose value is the one read.
    pub winner: PathBuf,
    /// The included file whose value was discarded.
    pub overridden: PathBuf,
}

/// One element of a TOML array. An array holds one kind of element.
#[derive(Debug, Clone, PartialEq)]
pub enum ListElement {
    /// A scalar element.
    Scalar(ParamValue),
    /// A table element. It has its own scalar keys, tables and lists.
    Table(Parameters),
}

impl ListElement {
    /// The scalar value, or nothing for a table element.
    #[must_use]
    pub fn scalar(&self) -> Option<&ParamValue> {
        match self {
            Self::Scalar(value) => Some(value),
            Self::Table(_) => None,
        }
    }

    /// The table, or nothing for a scalar element.
    #[must_use]
    pub fn table(&self) -> Option<&Parameters> {
        match self {
            Self::Table(params) => Some(params),
            Self::Scalar(_) => None,
        }
    }
}

/// The validated parameters one node was started with.
///
/// Scalars and tables are flat dotted names. A TOML array is a list, held apart from
/// the scalars; a list holds scalars or tables, never lists (a table element may hold lists of its own). Any duplicate across included files is refused.
///
/// `node`, `ports` and `log` are reserved. The `log` table is not a parameter and is
/// left out of the flat map; read it with `log_table`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parameters {
    values: BTreeMap<String, ParamValue>,
    lists: BTreeMap<String, Vec<ListElement>>,
    log: Option<Table>,
    /// The file that wrote each scalar and each list.
    origins: BTreeMap<String, PathBuf>,
    /// The keys of included files that the root file replaced, in file order.
    overrides: Vec<ParameterOverride>,
    read_values: ReadSet,
    read_lists: ReadSet,
    /// Boxed: a table list element holds a `Parameters`, and the set is empty there.
    ignored: Box<IgnoreSet>,
}

impl Parameters {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_sources(
        log: Option<Table>,
        origins: BTreeMap<String, PathBuf>,
        overrides: Vec<ParameterOverride>,
    ) -> Self {
        Self { log, origins, overrides, ..Self::default() }
    }

    /// The keys of included files that the root file replaced.
    #[must_use]
    pub fn overrides(&self) -> &[ParameterOverride] {
        &self.overrides
    }

    /// The directory of the file that wrote the value `name`, against which a relative
    /// file name in it resolves. `name` is a scalar, a list, or a list element or field
    /// (`sensors[0].file`). Nothing when not from a file. Does not mark `name` as read.
    #[must_use]
    pub fn origin_of(&self, name: &str) -> Option<&Path> {
        let owner = match name.find('[') {
            Some(bracket) => name.get(..bracket)?,
            None => name,
        };
        self.origins.get(owner).and_then(|file| file.parent())
    }

    pub(crate) fn insert_scalar(&mut self, key: String, value: ParamValue) -> Option<ParamValue> {
        self.values.insert(key, value)
    }

    pub(crate) fn insert_list(
        &mut self,
        key: String,
        elements: Vec<ListElement>,
    ) -> Option<Vec<ListElement>> {
        self.lists.insert(key, elements)
    }

    /// Load immutable parameters from a TOML file.
    ///
    /// A top-level `include` array lists other TOML files, resolved against the directory
    /// of the file that holds them. Tables merge key by key; duplicate scalars or lists
    /// are refused. Every string may contain `${NAME}` environment references.
    ///
    /// # Errors
    /// Returns an error when a file, an include, a value, or an environment
    /// reference is invalid, and when a referenced variable is unset or empty.
    pub fn from_toml_file(path: &Path) -> Result<Self, ParameterLoadError> {
        load_from_toml_file(path)
    }

    /// The `log` table of the parameter file, unchanged.
    #[must_use]
    pub fn log_table(&self) -> Option<&Table> {
        self.log.as_ref()
    }

    /// True when `name` is a scalar with a value, or a scalar of a table element
    /// (`sensors[1].topic`). Does not mark `name` as read.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.values.contains_key(name)
            || self.element_of(name, false).is_some_and(|(element, inner)| element.has(inner))
    }

    /// True when `name` is a list, or `list[index].inner` is a list of a table element.
    /// Does not mark `name` as read.
    #[must_use]
    pub fn has_list(&self, name: &str) -> bool {
        self.lists.contains_key(name)
            || self.element_of(name, false).is_some_and(|(element, inner)| element.has_list(inner))
    }

    /// The table element that `name` starts in, and the rest of `name` inside it:
    /// `sensors[1].gains` is element 1 of `sensors` and `gains`. Marks the element as
    /// read when `mark` is set.
    fn element_of<'s, 'n>(
        &'s self,
        name: &'n str,
        mark: bool,
    ) -> Option<(&'s Parameters, &'n str)> {
        let (list, index, inner) = split_element_name(name)?;
        let element = self.lists.get(list)?.get(index)?.table()?;
        if mark {
            self.read_lists.insert(list);
            self.read_values.insert(&format!("{list}[{index}]"));
        }
        Some((element, inner))
    }

    /// The names of the tables directly under `prefix` (`ports.inputs`; the empty prefix
    /// is the top level, and `sensors[0]` is a table element), sorted: `["a", "b"]` for
    /// `[ports.inputs.a]` and `[ports.inputs.b]`. A name that holds a scalar or a list is
    /// no table, and a table with no key is not there. Marks nothing as read.
    #[must_use]
    pub fn table_names(&self, prefix: &str) -> Vec<String> {
        if let Some((element, inner)) = self.element_of(prefix, false) {
            return element.table_names(inner);
        }
        let mut names = BTreeSet::new();
        for key in self.values.keys().chain(self.lists.keys()) {
            let below = if prefix.is_empty() {
                Some(key.as_str())
            } else {
                key.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('.'))
            };
            if let Some((table, _)) = below.and_then(|rest| rest.split_once('.')) {
                names.insert(table.to_string());
            }
        }
        names.into_iter().collect()
    }

    /// Each table element of every list, as `list[index]` and its parameters. Marks nothing.
    pub(crate) fn table_elements(&self) -> Vec<(String, &Parameters)> {
        let mut found = Vec::new();
        for (list, elements) in &self.lists {
            for (index, element) in elements.iter().enumerate() {
                if let ListElement::Table(table) = element {
                    found.push((format!("{list}[{index}]"), table));
                }
            }
        }
        found
    }

    /// True when the scalar `name` has been read. Does not mark `name` as read.
    #[must_use]
    pub fn is_read(&self, name: &str) -> bool {
        self.read_values.contains(name)
    }

    /// The value of the scalar `name`, if present; `name` may be a scalar of a table
    /// element (`sensors[1].topic`). Marks `name` as read when present.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&ParamValue> {
        let Some(found) = self.values.get(name) else {
            let (element, inner) = self.element_of(name, true)?;
            return element.value(inner);
        };
        self.read_values.insert(name);
        Some(found)
    }

    /// Every scalar parameter, ordered by dotted name. Lists are not included.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParamValue)> {
        self.values.iter().map(|(name, value)| (name.as_str(), value))
    }

    /// Every scalar name that has a value, in order. Lists are not included.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }

    /// The number of elements in the list `name`, which may be a list inside a table
    /// element (`sensors[1].gains`). Marks `name` as read when present.
    #[must_use]
    pub fn list_len(&self, name: &str) -> Option<usize> {
        let Some(list) = self.lists.get(name) else {
            let (element, inner) = self.element_of(name, true)?;
            return element.list_len(inner);
        };
        self.read_lists.insert(name);
        Some(list.len())
    }

    /// Element `index` of the list `name`, in TOML order; `name` may name a list inside a
    /// table element (`sensors[1].gains`). Marks the list and `name[index]` as read when
    /// present.
    #[must_use]
    pub fn list_element(&self, name: &str, index: usize) -> Option<&ListElement> {
        let Some(list) = self.lists.get(name) else {
            let (element, inner) = self.element_of(name, true)?;
            return element.list_element(inner, index);
        };
        let element = list.get(index)?;
        self.read_lists.insert(name);
        self.read_values.insert(&format!("{name}[{index}]"));
        Some(element)
    }

    /// Every list name, sorted. Scalar names are not included; see `names`.
    #[must_use]
    pub fn list_names(&self) -> Vec<&str> {
        self.lists.keys().map(String::as_str).collect()
    }

    /// Marks `key_or_prefix`, a key, a table or a list entry, as set on purpose and not
    /// read, for `reason`: it and every key under it leave the unread keys. A key the
    /// configuration does not set is accepted. True when it was not ignored before; a
    /// second call for the same key keeps the first reason.
    pub fn ignore(&self, key_or_prefix: &str, reason: &str) -> bool {
        self.ignored.insert(IgnoredKey {
            key: key_or_prefix.to_string(),
            reason: reason.to_string(),
            listed_in_file: false,
        })
    }

    pub(crate) fn ignore_listed(&self, key_or_prefix: &str, file: &Path) -> bool {
        self.ignored.insert(IgnoredKey {
            key: key_or_prefix.to_string(),
            reason: format!("listed in `unused` of {}", file.display()),
            listed_in_file: true,
        })
    }

    /// The ignored keys, file-listed ones first, then in the order code ignored them.
    #[must_use]
    pub fn ignored(&self) -> Vec<IgnoredKey> {
        self.ignored.snapshot()
    }

    /// Every configured key that has not been read yet and is not ignored, sorted.
    ///
    /// `node.name` is excluded. Table list elements report each field as `list[index].field`.
    #[must_use]
    pub fn unread_keys(&self) -> Vec<String> {
        let mut unread = self.unread_keys_inner(false);
        unread.retain(|key| !self.ignored.covers(key));
        unread
    }

    /// Every configured key while the loader builds the set, before anything reads it;
    /// `node.name` is excluded.
    pub(crate) fn configured_keys_at_load(&self) -> Vec<String> {
        self.unread_keys_inner(false)
    }

    /// Every unread key under `prefix`, sorted.
    #[must_use]
    pub fn unread_keys_under(&self, prefix: &str) -> Vec<String> {
        let all = self.unread_keys();
        if prefix.is_empty() {
            return all;
        }
        all.into_iter().filter(|key| key_is_under(key, prefix)).collect()
    }

    fn unread_keys_inner(&self, in_list_element: bool) -> Vec<String> {
        let mut unread = Vec::new();
        for key in self.values.keys() {
            if !in_list_element && RESERVED_SCALARS.contains(&key.as_str()) {
                continue;
            }
            if !self.read_values.contains(key) {
                unread.push(key.clone());
            }
        }
        for (list_name, elements) in &self.lists {
            let has_tables = elements.iter().any(|item| item.table().is_some());
            if has_tables {
                for (index, item) in elements.iter().enumerate() {
                    if let ListElement::Table(table) = item {
                        let is_empty = table.values.is_empty() && table.lists.is_empty();
                        if is_empty && !self.read_lists.contains(list_name) {
                            unread.push(format!("{list_name}[{index}]"));
                        } else {
                            for sub_key in table.unread_keys_inner(true) {
                                unread.push(format!("{list_name}[{index}].{sub_key}"));
                            }
                        }
                    }
                }
            } else if !self.read_lists.contains(list_name) {
                unread.push(list_name.clone());
            } else {
                for index in 0..elements.len() {
                    let indexed = format!("{list_name}[{index}]");
                    if !self.read_values.contains(&indexed) {
                        unread.push(indexed);
                    }
                }
            }
        }
        unread.sort();
        unread.dedup();
        unread
    }
}
