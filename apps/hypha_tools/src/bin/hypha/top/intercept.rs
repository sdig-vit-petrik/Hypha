//! The Intercept tab's model: a frontend for `hypha intercept` backends.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use hypha_node::sdk::message::DecodeOwned as _;
use hypha_tools::intercept_doc::{
    BackendState, InterceptConfig, InterceptionSpec, InterceptionState, OverrideSpec, load_config,
    parse_config, write_config_atomic,
};
use hypha_tools::msg_patch::{
    Radix, ScalarValue, escape, format_value, integer_width, parse_value, read_scalar,
};
use hypha_tools::msg_reflect::{MessageSchema, WalkEvent, is_byte_array, walk};

use crate::top::state::Topics;

/// A backend whose state has not arrived for this long is shown offline.
pub(crate) const OFFLINE_AFTER: Duration = Duration::from_secs(2);
/// Arrays with more elements start collapsed.
pub(crate) const COLLAPSE_ABOVE: usize = 16;
const STATE_PREFIX: &str = "/hypha/intercept/";
const STATE_SUFFIX: &str = "/state";

/// The pane that takes the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    List,
    Tree,
    Editor,
}

impl Pane {
    fn next(self) -> Self {
        match self {
            Self::List => Self::Tree,
            Self::Tree => Self::Editor,
            Self::Editor => Self::List,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::List => Self::Editor,
            Self::Tree => Self::List,
            Self::Editor => Self::Tree,
        }
    }
}

/// The tab layout for a terminal size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Two columns: list and details, fields and override (≥ 110 columns).
    Wide,
    /// A one-row selector above fields and override (80–109 columns).
    Medium,
    /// One pane at a time with a breadcrumb (60–79 columns).
    Narrow,
    /// Below 60×12: only a notice.
    TooSmall,
}

/// The layout for a terminal of `width`×`height`.
#[must_use]
pub(crate) fn layout_for(width: u16, height: u16) -> Layout {
    match width {
        _ if width < 60 || height < 12 => Layout::TooSmall,
        110.. => Layout::Wide,
        80.. => Layout::Medium,
        _ => Layout::Narrow,
    }
}

/// One bit of the tri-state grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitState {
    Keep,
    Set,
    Clear,
}

impl BitState {
    fn next(self) -> Self {
        match self {
            Self::Keep => Self::Set,
            Self::Set => Self::Clear,
            Self::Clear => Self::Keep,
        }
    }
}

/// What a tree row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowKind {
    /// A nested message, or an element of a message array.
    Message,
    /// An array of `count` elements.
    Array { count: usize },
    /// A byte array, shown as its size; it cannot be overridden.
    Bytes { count: usize },
    /// A builtin value at `span` of the input payload.
    Scalar { type_name: String, span: Range<usize> },
}

/// One row of the field tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeRow {
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub kind: RowKind,
    /// For a message or array row: its children are shown.
    pub open: bool,
    /// The row opens and closes (a field, not an array element).
    pub toggleable: bool,
}

/// The last component of a path: `voltage`, `cells`, or `[2]` for an element.
fn label_of(path: &str) -> String {
    let tail = path.rsplit('.').next().unwrap_or(path);
    match tail.rfind('[') {
        Some(index) if tail.ends_with(']') && index > 0 => tail.get(index..).unwrap_or(tail),
        _ => tail,
    }
    .to_owned()
}

fn depth_of(path: &str) -> usize {
    path.bytes().filter(|byte| matches!(byte, b'.' | b'[')).count()
}

fn parent_of(path: &str) -> Option<String> {
    let cut = path.rfind(['.', '['])?;
    path.get(..cut).map(str::to_owned)
}

struct Frame {
    path: String,
    closed: bool,
    array: bool,
    bytes: bool,
    element: Option<String>,
}

/// The rows of the field tree of `payload`, a `root` message. A message row is open
/// unless `toggled` names it; an array is open when it has at most
/// [`COLLAPSE_ABOVE`] elements, the other way round when `toggled` names it.
///
/// # Errors
/// Returns the reason when the payload does not match the schema.
pub(crate) fn build_rows(
    schema: &mut MessageSchema,
    root: &str,
    payload: &[u8],
    toggled: &BTreeSet<String>,
) -> Result<Vec<TreeRow>, String> {
    let mut rows = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut unresolved = None;
    let hidden = |stack: &[Frame]| stack.iter().any(|frame| frame.closed);
    let prefix = |stack: &[Frame]| match stack.last() {
        Some(frame) if frame.array => frame.element.clone().unwrap_or_else(|| frame.path.clone()),
        Some(frame) => frame.path.clone(),
        None => String::new(),
    };
    walk(schema, root, payload, &mut |event| match event {
        WalkEvent::Enter { field } => {
            let parent = prefix(&stack);
            let path = if parent.is_empty() {
                field.name.clone()
            } else {
                format!("{parent}.{}", field.name)
            };
            let message =
                !field.is_array && !hypha_tools::msg_reflect::is_builtin_type(&field.type_name);
            let mut closed = false;
            if message {
                let open = !toggled.contains(&path);
                closed = !open;
                if !hidden(&stack) {
                    rows.push(TreeRow {
                        label: label_of(&path),
                        depth: depth_of(&path),
                        path: path.clone(),
                        kind: RowKind::Message,
                        open,
                        toggleable: true,
                    });
                }
            }
            stack.push(Frame {
                path,
                closed,
                array: field.is_array,
                bytes: is_byte_array(field),
                element: None,
            });
        }
        WalkEvent::ArrayStart { count, .. } => {
            let enclosing = stack.get(..stack.len().saturating_sub(1)).unwrap_or(&[]);
            let visible = !hidden(enclosing);
            let Some(frame) = stack.last_mut() else { return };
            let open = (count <= COLLAPSE_ABOVE) != toggled.contains(&frame.path);
            frame.closed = !open || frame.bytes;
            if visible {
                let kind =
                    if frame.bytes { RowKind::Bytes { count } } else { RowKind::Array { count } };
                rows.push(TreeRow {
                    label: label_of(&frame.path),
                    depth: depth_of(&frame.path),
                    path: frame.path.clone(),
                    kind,
                    open: open && !frame.bytes,
                    toggleable: !frame.bytes,
                });
            }
        }
        WalkEvent::Element { index } => {
            if let Some(frame) = stack.last_mut() {
                frame.element = Some(format!("{}[{index}]", frame.path));
            }
        }
        WalkEvent::Scalar { type_name, span } => {
            if hidden(&stack) {
                return;
            }
            let Some(frame) = stack.last() else { return };
            let path = frame.element.clone().unwrap_or_else(|| frame.path.clone());
            rows.push(TreeRow {
                label: label_of(&path),
                depth: depth_of(&path),
                path,
                kind: RowKind::Scalar { type_name: type_name.to_owned(), span },
                open: false,
                toggleable: false,
            });
        }
        WalkEvent::Leave => {
            stack.pop();
        }
        WalkEvent::Unresolved { type_name } => {
            unresolved.get_or_insert_with(|| format!("type {type_name} is not defined"));
        }
        WalkEvent::Bytes { .. } | WalkEvent::ArrayEnd => {}
    })
    .map_err(|cause| format!("payload does not match the schema: {cause}"))?;
    if let Some(reason) = unresolved {
        return Err(reason);
    }
    Ok(insert_element_rows(rows))
}

/// Adds a `[i]` header row before the first field of each message-array element.
fn insert_element_rows(rows: Vec<TreeRow>) -> Vec<TreeRow> {
    let mut out: Vec<TreeRow> = Vec::with_capacity(rows.len());
    let mut headers = BTreeSet::new();
    for row in rows {
        if let Some(element) = element_of(&row.path)
            && headers.insert(element.clone())
        {
            out.push(TreeRow {
                label: label_of(&element),
                depth: depth_of(&element),
                path: element,
                kind: RowKind::Message,
                open: true,
                toggleable: false,
            });
        }
        out.push(row);
    }
    out
}

/// The innermost `x[i]` prefix of `path` that is followed by a field.
fn element_of(path: &str) -> Option<String> {
    let close = path.rfind("].")?;
    path.get(..=close).map(str::to_owned)
}

/// An override being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditBuffer {
    pub path: String,
    pub type_name: String,
    pub text: String,
    /// Cursor position, in characters.
    pub cursor: usize,
    /// The selected bit of the grid, 0 = least significant.
    pub bit: usize,
    /// One entry per bit for an integer; empty for any other type.
    pub bits: Vec<BitState>,
    /// Why the last apply was refused.
    pub error: String,
}

/// Topic hints listed under a topic field of the new-interception form.
pub(crate) const SUGGESTIONS: usize = 8;

/// A field of the new-interception form, in the order Tab visits them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FormField {
    #[default]
    Input,
    Output,
    Name,
}

impl FormField {
    pub(crate) const ALL: [Self; 3] = [Self::Input, Self::Output, Self::Name];

    fn next(self) -> Self {
        match self {
            Self::Input => Self::Output,
            Self::Output => Self::Name,
            Self::Name => Self::Input,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Input => Self::Name,
            Self::Output => Self::Input,
            Self::Name => Self::Output,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Name => "name",
        }
    }
}

/// The modal `n` opens. Output and name follow the input until they are typed in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NewForm {
    pub input: String,
    pub output: String,
    pub name: String,
    pub field: FormField,
    /// The highlighted topic hint.
    pub pick: Option<usize>,
    output_typed: bool,
    name_typed: bool,
    pub error: String,
}

impl NewForm {
    fn new(input: &str) -> Self {
        let mut form = Self::default();
        form.set_input(input.to_owned());
        form
    }

    pub(crate) fn text(&self, field: FormField) -> &str {
        match field {
            FormField::Input => &self.input,
            FormField::Output => &self.output,
            FormField::Name => &self.name,
        }
    }

    fn set_input(&mut self, input: String) {
        if !self.output_typed {
            self.output =
                if input.is_empty() { String::new() } else { format!("{input}/intercepted") };
        }
        if !self.name_typed {
            self.name = name_from_topic(&input);
        }
        self.input = input;
    }

    /// Changes the focused field's text.
    fn edit(&mut self, change: impl FnOnce(&mut String)) {
        self.pick = None;
        match self.field {
            FormField::Input => {
                let mut input = self.input.clone();
                change(&mut input);
                self.set_input(input);
            }
            FormField::Output => {
                change(&mut self.output);
                self.output_typed = true;
            }
            FormField::Name => {
                change(&mut self.name);
                self.name_typed = true;
            }
        }
    }
}

/// An interception name for `topic`: `/battery/raw` → `battery_raw`.
fn name_from_topic(topic: &str) -> String {
    let mut name = String::new();
    for character in topic.chars().map(|character| character.to_ascii_lowercase()) {
        if character.is_ascii_alphanumeric() {
            name.push(character);
        } else if !name.is_empty() && !name.ends_with('_') {
            name.push('_');
        }
    }
    name.trim_end_matches('_').to_owned()
}

/// The topic hints of the focused form field: the topics on the bus whose name
/// contains the typed text, prefix matches first, each with its type. The name
/// field has none.
pub(crate) fn suggestions<'t>(form: &NewForm, topics: &'t Topics) -> Vec<(&'t str, &'t str)> {
    if form.field == FormField::Name {
        return Vec::new();
    }
    let typed = form.text(form.field).to_lowercase();
    // A name of several streams is one hint, with the type of its first.
    let mut hits: Vec<(bool, &str, &str)> = topics
        .names()
        .filter(|topic| !is_state_topic(topic))
        .filter_map(|topic| {
            let lower = topic.to_lowercase();
            let data_type = topics.get(topic).map_or("", |stats| stats.data_type.as_str());
            lower.contains(&typed).then(|| (!lower.starts_with(&typed), topic, data_type))
        })
        .collect();
    hits.sort_by_key(|(later, _, _)| *later);
    hits.into_iter().take(SUGGESTIONS).map(|(_, topic, data_type)| (topic, data_type)).collect()
}

/// The status bar contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatusLine {
    /// The selected interception's arm switch, `None` without one.
    pub armed: Option<bool>,
    pub last_error: String,
    pub pending: bool,
    pub offline: bool,
    pub config_error: String,
    /// The last refused edit or write.
    pub message: String,
}

/// The Intercept tab.
#[derive(Debug, Default)]
pub(crate) struct InterceptView {
    /// The canonical directory the config files may be read and written under; `None`
    /// refuses every path.
    pub intercept_dir: Option<PathBuf>,
    backends: BTreeMap<String, (BackendState, Instant)>,
    /// State documents that did not decode.
    pub bad_state_docs: u64,
    /// The config file as last read.
    pub config: InterceptConfig,
    pub selected: Option<String>,
    pub focus: Option<Pane>,
    pub selected_path: Option<String>,
    pub toggled: BTreeSet<String>,
    pub edit: Option<EditBuffer>,
    pub form: Option<NewForm>,
    pub confirm_delete: bool,
    /// The field tree of the selected interception, set by the drawing code.
    pub rows: Vec<TreeRow>,
    pub message: String,
    written_revision: u64,
}

/// True for the topic of a backend state document.
pub(crate) fn is_state_topic(topic: &str) -> bool {
    topic
        .strip_prefix(STATE_PREFIX)
        .and_then(|rest| rest.strip_suffix(STATE_SUFFIX))
        .is_some_and(|name| !name.is_empty() && !name.contains('/'))
}

/// The canonical path of the file `path` names, when it lies under `dir` once `..` and
/// symlinks are resolved; `None` otherwise. The caller reads and writes the returned path,
/// never the path the bus named.
pub(crate) fn confined_path(dir: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let file_name = path.file_name()?;
    let parent = path.parent()?.canonicalize().ok()?;
    let named = parent.join(file_name);
    // A file that exists is resolved through its symlinks. One that does not exist, or
    // is a dangling link, keeps the name in the canonical parent: a write renames over
    // the link and leaves its target alone.
    let resolved = std::fs::canonicalize(path).unwrap_or(named);
    let inside: bool = resolved.starts_with(dir);
    inside.then_some(resolved)
}

impl InterceptView {
    /// The pane with the keys.
    pub(crate) fn pane(&self) -> Pane {
        self.focus.unwrap_or(Pane::List)
    }

    /// Takes one state document.
    pub(crate) fn on_state(&mut self, payload: &[u8], now: Instant) {
        let decoded = hypha_node::sdk::msgs::std_msgs::String::decode_from(payload)
            .map_err(|error| error.to_string())
            .and_then(|message| {
                toml::from_str::<BackendState>(&message.data).map_err(|error| error.to_string())
            });
        let Ok(state) = decoded else {
            self.bad_state_docs = self.bad_state_docs.saturating_add(1);
            return;
        };
        // The backend is shown even when its config path is refused, but the path is not read.
        let guarded = self.guarded_path(&state.config_path);
        self.backends.insert(state.backend.clone(), (state, now));
        match guarded.and_then(|path| load_config(&path)) {
            Ok(config) => self.config = config,
            Err(refusal_or_error) => self.message = refusal_or_error,
        }
    }

    /// The path of a backend's config file, resolved under the intercept directory, or the
    /// refusal that says why it is not used.
    fn guarded_path(&self, config_path: &str) -> Result<PathBuf, String> {
        let Some(dir) = &self.intercept_dir else {
            return Err(format!("refused: {config_path} is outside the intercept directory"));
        };
        confined_path(dir, Path::new(config_path))
            .ok_or_else(|| format!("refused: {config_path} is outside {}", dir.display()))
    }

    /// The first backend by name is the one shown.
    pub(crate) fn backend(&self) -> Option<&BackendState> {
        self.backends.values().next().map(|(state, _)| state)
    }

    fn backend_seen(&self) -> Option<Instant> {
        self.backends.values().next().map(|(_, seen)| *seen)
    }

    /// The selected interception as the file describes it.
    pub(crate) fn spec(&self) -> Option<&InterceptionSpec> {
        let specs = &self.config.interception;
        match &self.selected {
            Some(name) => specs.iter().find(|spec| &spec.name == name).or_else(|| specs.first()),
            None => specs.first(),
        }
    }

    /// The backend's state of interception `name`.
    pub(crate) fn applied(&self, name: &str) -> Option<&InterceptionState> {
        self.backend()?.interception.iter().find(|state| state.name == name)
    }

    pub(crate) fn status(&self, now: Instant) -> StatusLine {
        let spec = self.spec();
        let applied = spec.and_then(|spec| self.applied(&spec.name));
        let backend = self.backend();
        let offline = self
            .backend_seen()
            .is_none_or(|seen| now.saturating_duration_since(seen) >= OFFLINE_AFTER);
        StatusLine {
            armed: spec.map(|spec| spec.armed),
            last_error: applied.map(|state| state.last_error.clone()).unwrap_or_default(),
            pending: backend.is_some_and(|state| state.applied_revision < self.written_revision),
            offline,
            config_error: backend.map(|state| state.config_error.clone()).unwrap_or_default(),
            message: self.message.clone(),
        }
    }

    /// The selected row of the tree.
    pub(crate) fn selected_row(&self) -> Option<&TreeRow> {
        let path = self.selected_path.as_ref()?;
        self.rows.iter().find(|row| &row.path == path)
    }

    /// Handles one key of the Intercept tab. `true` when the tab used it; `false`
    /// leaves it to the global bindings.
    pub(crate) fn handle_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        topics: &Topics,
        default_input: Option<&str>,
    ) -> bool {
        if self.form.is_some() {
            self.form_key(code, topics);
            return true;
        }
        if self.confirm_delete {
            self.confirm_delete = false;
            if code == KeyCode::Char('y') {
                self.delete_selected();
            }
            return true;
        }
        let pane = self.pane();
        match (pane, code) {
            (_, KeyCode::Tab) => self.focus_pane(pane.next(), topics),
            (_, KeyCode::BackTab) => self.focus_pane(pane.previous(), topics),
            (Pane::Editor, _) => return self.editor_key(code, modifiers),
            (Pane::List, KeyCode::Up | KeyCode::Char('k')) => self.select_offset(false),
            (Pane::List, KeyCode::Down | KeyCode::Char('j')) => self.select_offset(true),
            (Pane::List, KeyCode::Enter) => self.focus = Some(Pane::Tree),
            (Pane::List, KeyCode::Char('n')) => {
                self.form = Some(NewForm::new(default_input.unwrap_or_default()));
            }
            (Pane::List | Pane::Tree, KeyCode::Char('d') | KeyCode::Delete) => {
                self.confirm_delete = self.spec().is_some();
            }
            (Pane::List | Pane::Tree, KeyCode::Char('a')) => self.toggle_armed(),
            (Pane::Tree, KeyCode::Up | KeyCode::Char('k')) => self.move_row(false),
            (Pane::Tree, KeyCode::Down | KeyCode::Char('j')) => self.move_row(true),
            (Pane::Tree, KeyCode::Left | KeyCode::Char('h')) => self.collapse(),
            (Pane::Tree, KeyCode::Right | KeyCode::Char('l')) => self.set_open(true),
            (Pane::Tree, KeyCode::Enter) => {
                let scalar = matches!(
                    self.selected_row().map(|row| &row.kind),
                    Some(RowKind::Scalar { .. })
                );
                if scalar {
                    self.focus_pane(Pane::Editor, topics);
                } else {
                    let open = self.selected_row().is_some_and(|row| row.open);
                    self.set_open(!open);
                }
            }
            (Pane::Tree, KeyCode::Esc) => self.focus = Some(Pane::List),
            (Pane::Tree, KeyCode::Char('x')) => self.clear_selected_override(),
            (Pane::Tree, KeyCode::Char('X')) => self.clear_all_overrides(),
            _ => return false,
        }
        true
    }

    fn focus_pane(&mut self, pane: Pane, topics: &Topics) {
        self.focus = Some(pane);
        if pane == Pane::Editor {
            self.open_edit(topics);
        }
    }

    fn select_offset(&mut self, forward: bool) {
        let names: Vec<String> =
            self.config.interception.iter().map(|spec| spec.name.clone()).collect();
        let current = self.spec().and_then(|spec| names.iter().position(|name| name == &spec.name));
        let next = step_index(current, forward, names.len());
        if let Some(name) = names.get(next)
            && self.selected.as_ref() != Some(name)
        {
            self.selected = Some(name.clone());
            self.selected_path = None;
            self.toggled.clear();
            self.edit = None;
        }
    }

    fn move_row(&mut self, forward: bool) {
        let current = self
            .selected_path
            .as_ref()
            .and_then(|path| self.rows.iter().position(|row| &row.path == path));
        let next = step_index(current, forward, self.rows.len());
        if let Some(row) = self.rows.get(next) {
            self.selected_path = Some(row.path.clone());
        }
    }

    fn set_open(&mut self, open: bool) {
        let Some(row) = self.selected_row() else { return };
        if row.toggleable && row.open != open {
            let path = row.path.clone();
            if !self.toggled.remove(&path) {
                self.toggled.insert(path);
            }
        }
    }

    fn collapse(&mut self) {
        let Some(row) = self.selected_row() else { return };
        if row.toggleable && row.open {
            self.set_open(false);
        } else if let Some(parent) = parent_of(&row.path) {
            self.selected_path = Some(parent);
        }
    }

    fn open_edit(&mut self, topics: &Topics) {
        let Some(TreeRow { path, kind: RowKind::Scalar { type_name, span }, .. }) =
            self.selected_row().cloned()
        else {
            return;
        };
        if self.edit.as_ref().is_some_and(|edit| edit.path == path) {
            return;
        }
        let spec = self.spec();
        let stored = spec.and_then(|spec| spec.overrides.get(&path)).cloned();
        let current = spec
            .and_then(|spec| topics.get(&spec.input))
            .and_then(|stats| stats.last_payload.get(span))
            .and_then(|wire| read_scalar(&type_name, wire).ok());
        let width = integer_width(&type_name).map_or(0, |bytes| bytes.saturating_mul(8));
        let mut bits = vec![BitState::Keep; width];
        let mut text = current.as_ref().map(edit_text).unwrap_or_default();
        match stored {
            Some(OverrideSpec::Value { value }) => text = value,
            Some(OverrideSpec::Bits { set, clear }) => {
                let mask = |text: &str| match parse_value("uint64", text) {
                    Ok(ScalarValue::UInt(bits)) => bits,
                    _ => 0,
                };
                let (set, clear) = (mask(&set), mask(&clear));
                for (index, state) in bits.iter_mut().enumerate() {
                    let bit = bit_mask(index);
                    if set & bit != 0 {
                        *state = BitState::Set;
                    } else if clear & bit != 0 {
                        *state = BitState::Clear;
                    }
                }
            }
            None => {}
        }
        let cursor = text.chars().count();
        self.edit =
            Some(EditBuffer { path, type_name, text, cursor, bit: 0, bits, error: String::new() });
    }

    fn editor_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        let Some(edit) = self.edit.as_mut() else {
            if code == KeyCode::Esc {
                self.focus = Some(Pane::Tree);
                return true;
            }
            return false;
        };
        let integer = !edit.bits.is_empty();
        match code {
            KeyCode::Esc => {
                self.edit = None;
                self.focus = Some(Pane::Tree);
            }
            KeyCode::Enter => self.apply_value(),
            KeyCode::Char('x' | 'X') if modifiers.contains(KeyModifiers::CONTROL) => {
                let path = edit.path.clone();
                self.edit = None;
                self.remove_override(&path);
            }
            KeyCode::Char(' ') if integer => {
                if let Some(state) = edit.bits.get_mut(edit.bit) {
                    *state = state.next();
                }
                self.write_bits();
            }
            KeyCode::Char(' ') if edit.type_name == "bool" => {
                let flipped = if edit.text == "true" { "false" } else { "true" };
                flipped.clone_into(&mut edit.text);
                edit.cursor = edit.text.chars().count();
                self.apply_value();
            }
            KeyCode::Left if integer => {
                edit.bit = edit.bit.saturating_add(1).min(edit.bits.len().saturating_sub(1));
            }
            KeyCode::Right if integer => edit.bit = edit.bit.saturating_sub(1),
            KeyCode::Left => edit.cursor = edit.cursor.saturating_sub(1),
            KeyCode::Right => {
                edit.cursor = edit.cursor.saturating_add(1).min(edit.text.chars().count());
            }
            KeyCode::Home => edit.cursor = 0,
            KeyCode::End => edit.cursor = edit.text.chars().count(),
            KeyCode::Backspace => {
                if edit.cursor > 0 {
                    edit.cursor = edit.cursor.saturating_sub(1);
                    remove_char(&mut edit.text, edit.cursor);
                }
            }
            KeyCode::Char(character) => {
                insert_char(&mut edit.text, edit.cursor, character);
                edit.cursor = edit.cursor.saturating_add(1);
            }
            _ => return false,
        }
        true
    }

    fn apply_value(&mut self) {
        let Some(edit) = self.edit.as_mut() else { return };
        if let Err(error) = parse_value(&edit.type_name, &edit.text) {
            edit.error = error;
            return;
        }
        edit.error.clear();
        let (path, value) = (edit.path.clone(), edit.text.clone());
        self.set_override(&path, OverrideSpec::Value { value });
    }

    fn write_bits(&mut self) {
        let Some(edit) = self.edit.as_ref() else { return };
        let (mut set, mut clear) = (0_u64, 0_u64);
        for (index, state) in edit.bits.iter().enumerate() {
            let bit = bit_mask(index);
            match state {
                BitState::Set => set |= bit,
                BitState::Clear => clear |= bit,
                BitState::Keep => {}
            }
        }
        let path = edit.path.clone();
        if set == 0 && clear == 0 {
            self.remove_override(&path);
        } else {
            let spec =
                OverrideSpec::Bits { set: format!("{set:#x}"), clear: format!("{clear:#x}") };
            self.set_override(&path, spec);
        }
    }

    fn set_override(&mut self, path: &str, spec: OverrideSpec) {
        let path = path.to_owned();
        self.mutate_spec(|entry| {
            entry.overrides.insert(path, spec);
        });
    }

    fn remove_override(&mut self, path: &str) {
        self.mutate_spec(|entry| {
            entry.overrides.remove(path);
        });
    }

    fn clear_selected_override(&mut self) {
        if let Some(path) = self.selected_path.clone() {
            if self.edit.as_ref().is_some_and(|edit| edit.path == path) {
                self.edit = None;
            }
            self.remove_override(&path);
        }
    }

    fn clear_all_overrides(&mut self) {
        if self.spec().is_some() {
            self.edit = None;
        }
        self.mutate_spec(|entry| entry.overrides.clear());
    }

    fn toggle_armed(&mut self) {
        self.mutate_spec(|entry| entry.armed = !entry.armed);
    }

    /// Applies `change` to the selected interception in the file; does nothing without one.
    fn mutate_spec(&mut self, change: impl FnOnce(&mut InterceptionSpec)) {
        let Some(name) = self.spec().map(|spec| spec.name.clone()) else { return };
        self.mutate(|config| {
            change(find(config, &name)?);
            Ok(())
        });
    }

    fn delete_selected(&mut self) {
        let Some(name) = self.spec().map(|spec| spec.name.clone()) else { return };
        self.mutate(|config| {
            config.interception.retain(|spec| spec.name != name);
            Ok(())
        });
        self.selected = None;
        self.selected_path = None;
        self.edit = None;
    }

    fn form_key(&mut self, code: KeyCode, topics: &Topics) {
        let Some(form) = self.form.as_mut() else { return };
        let hints: Vec<String> =
            suggestions(form, topics).into_iter().map(|(topic, _)| topic.to_owned()).collect();
        let picked = form.pick.and_then(|index| hints.get(index)).cloned();
        match code {
            KeyCode::Esc if form.pick.is_some() => form.pick = None,
            KeyCode::Esc => self.form = None,
            KeyCode::Down if !hints.is_empty() => {
                let next = form.pick.map_or(0, |index| index.saturating_add(1));
                form.pick = Some(next.min(hints.len().saturating_sub(1)));
            }
            KeyCode::Up => form.pick = form.pick.and_then(|index| index.checked_sub(1)),
            KeyCode::Enter if picked.is_some() => {
                form.edit(|text| *text = picked.unwrap_or_default());
            }
            KeyCode::Tab => {
                if let Some(topic) = picked {
                    form.edit(|text| *text = topic);
                }
                form.field = form.field.next();
                form.pick = None;
            }
            KeyCode::BackTab => {
                form.field = form.field.previous();
                form.pick = None;
            }
            KeyCode::Backspace => form.edit(|text| {
                text.pop();
            }),
            KeyCode::Char(character) => form.edit(|text| text.push(character)),
            KeyCode::Enter => self.submit_form(),
            _ => {}
        }
    }

    fn submit_form(&mut self) {
        let Some(form) = self.form.as_ref() else { return };
        let name = form.name.clone();
        let spec = InterceptionSpec {
            name: name.clone(),
            input: form.input.clone(),
            output: form.output.clone(),
            armed: true,
            overrides: BTreeMap::new(),
        };
        self.message.clear();
        self.mutate(|config| {
            config.interception.push(spec);
            Ok(())
        });
        if self.message.is_empty() {
            self.form = None;
            self.selected = Some(name);
            self.selected_path = None;
            self.toggled.clear();
            self.edit = None;
        } else if let Some(form) = self.form.as_mut() {
            form.error = std::mem::take(&mut self.message);
        }
    }

    /// Reads the file, changes it, checks it, bumps its revision past both the file's
    /// and the applied one, and writes it atomically. A refusal lands in `message`
    /// and changes nothing; a file that does not parse is never overwritten.
    fn mutate(&mut self, change: impl FnOnce(&mut InterceptConfig) -> Result<(), String>) {
        let Some(backend) = self.backend() else {
            "no hypha intercept backend is running".clone_into(&mut self.message);
            return;
        };
        let applied = backend.applied_revision;
        let guarded = self.guarded_path(&backend.config_path);
        let written = guarded.and_then(|path| {
            let mut config = load_config(&path)?;
            change(&mut config)?;
            config.revision = config.revision.max(applied).saturating_add(1);
            let text = toml::to_string(&config).map_err(|error| error.to_string())?;
            parse_config(&text)?;
            write_config_atomic(&path, &config)?;
            Ok(config)
        });
        match written {
            Ok(config) => {
                self.written_revision = config.revision;
                self.config = config;
                self.message.clear();
            }
            Err(error) => self.message = error,
        }
    }
}

/// The row after (`forward`) or before `current`, clamped to `len` rows; the first row when
/// nothing is current.
fn step_index(current: Option<usize>, forward: bool, len: usize) -> usize {
    match (current, forward) {
        (None, _) => 0,
        (Some(index), true) => index.saturating_add(1).min(len.saturating_sub(1)),
        (Some(index), false) => index.saturating_sub(1),
    }
}

/// Bit `index` of a 64-bit mask; zero past the last bit.
fn bit_mask(index: usize) -> u64 {
    u32::try_from(index).ok().and_then(|shift| 1_u64.checked_shl(shift)).unwrap_or(0)
}

fn find<'c>(
    config: &'c mut InterceptConfig,
    name: &str,
) -> Result<&'c mut InterceptionSpec, String> {
    config
        .interception
        .iter_mut()
        .find(|spec| spec.name == name)
        .ok_or_else(|| format!("interception '{name}' is no longer in the file"))
}

/// The text an edit starts from: the value as an override would spell it.
fn edit_text(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Text(bytes) => escape(bytes),
        other => format_value(other, Radix::Dec),
    }
}

pub(crate) fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(index, _)| index)
}

fn insert_char(text: &mut String, at: usize, character: char) {
    let index = byte_index(text, at);
    text.insert(index, character);
}

fn remove_char(text: &mut String, at: usize) {
    let index = byte_index(text, at);
    if index < text.len() {
        text.remove(index);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
pub(crate) mod tests {
    use super::*;
    use crate::top::state::AppState;
    use hypha_node::sdk::message::EncodeInto as _;
    use hypha_tools::intercept_doc::InterceptionState;

    pub(crate) const ROOT: &str = "test_msgs/Battery";
    pub(crate) const DEFINITION: &str = "uint16 status\nstring serial_number\ntest_msgs/Cell[] cells\n\
================================================================================\n\
MSG: test_msgs/Cell\nfloat32 voltage\n";
    pub(crate) const INPUT: &str = "/battery/raw";

    pub(crate) fn payload(cells: usize) -> Vec<u8> {
        let mut wire = 0x00F0_u16.to_le_bytes().to_vec();
        wire.extend_from_slice(&2_u32.to_le_bytes());
        wire.extend_from_slice(b"AB");
        wire.extend_from_slice(&u32::try_from(cells).unwrap().to_le_bytes());
        for _ in 0..cells {
            wire.extend_from_slice(&3.5_f32.to_le_bytes());
        }
        wire
    }

    pub(crate) fn state_doc(config_path: &std::path::Path, applied: u64) -> Vec<u8> {
        let state = BackendState {
            backend: "bench".to_owned(),
            config_path: config_path.display().to_string(),
            applied_revision: applied,
            config_error: String::new(),
            interception: vec![InterceptionState {
                name: "battery".to_owned(),
                ..InterceptionState::default()
            }],
        };
        let message =
            hypha_node::sdk::msgs::std_msgs::String { data: toml::to_string(&state).unwrap() };
        let mut buffer = vec![0; message.encoded_len()];
        message.encode_into(&mut buffer).unwrap();
        buffer
    }

    /// A view on a fresh config file with one interception, its tree built from a
    /// 3-cell payload, and the app state that holds the input traffic.
    pub(crate) fn bench(test: &str) -> (InterceptView, AppState, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("hypha_top_intercept_{test}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("intercept.toml");
        let config = parse_config(
            "revision = 3\n[[interception]]\nname = 'battery'\ninput = '/battery/raw'\noutput = '/battery'\n",
        )
        .unwrap();
        write_config_atomic(&path, &config).unwrap();
        let mut view = InterceptView {
            intercept_dir: Some(dir.canonicalize().unwrap()),
            ..InterceptView::default()
        };
        view.on_state(&state_doc(&path, 3), Instant::now());
        let mut app = AppState::new();
        app.on_sample(INPUT, 1, &payload(3));
        let mut schema = MessageSchema::from_definition(ROOT, DEFINITION).unwrap();
        view.rows = build_rows(&mut schema, ROOT, &payload(3), &view.toggled).unwrap();
        (view, app, path)
    }

    fn press(view: &mut InterceptView, app: &AppState, code: KeyCode) -> bool {
        view.handle_key(code, KeyModifiers::NONE, &app.topics, Some("/picked"))
    }

    fn type_text(view: &mut InterceptView, app: &AppState, text: &str) {
        for character in text.chars() {
            press(view, app, KeyCode::Char(character));
        }
    }

    fn file(path: &std::path::Path) -> InterceptConfig {
        load_config(path).unwrap()
    }

    fn select(view: &mut InterceptView, path: &str) {
        view.selected_path = Some(path.to_owned());
    }

    fn rebuild(view: &mut InterceptView) {
        let mut schema = MessageSchema::from_definition(ROOT, DEFINITION).unwrap();
        view.rows = build_rows(&mut schema, ROOT, &payload(3), &view.toggled).unwrap();
    }

    #[test]
    fn rows_name_paths_and_collapse_large_arrays() {
        let mut schema = MessageSchema::from_definition(ROOT, DEFINITION).unwrap();
        let rows = build_rows(&mut schema, ROOT, &payload(2), &BTreeSet::new()).unwrap();
        let paths: Vec<(&str, &str, usize)> =
            rows.iter().map(|row| (row.path.as_str(), row.label.as_str(), row.depth)).collect();
        assert_eq!(
            paths,
            [
                ("status", "status", 0),
                ("serial_number", "serial_number", 0),
                ("cells", "cells", 0),
                ("cells[0]", "[0]", 1),
                ("cells[0].voltage", "voltage", 2),
                ("cells[1]", "[1]", 1),
                ("cells[1].voltage", "voltage", 2),
            ]
        );
        let large = build_rows(&mut schema, ROOT, &payload(17), &BTreeSet::new()).unwrap();
        assert_eq!(large.len(), 3, "a 17-element array starts collapsed");
        assert!(!large[2].open);
        let opened =
            build_rows(&mut schema, ROOT, &payload(17), &BTreeSet::from(["cells".to_owned()]))
                .unwrap();
        assert_eq!(opened.len(), 3 + 17 * 2);
        let sixteen = build_rows(&mut schema, ROOT, &payload(16), &BTreeSet::new()).unwrap();
        assert_eq!(sixteen.len(), 3 + 16 * 2, "a 16-element array starts open");
    }

    #[test]
    fn keys_follow_the_key_table() {
        let (mut view, app, path) = bench("keys");
        assert_eq!(view.pane(), Pane::List);
        // List: Enter focuses the tree, Tab cycles, Shift-Tab goes back.
        assert!(press(&mut view, &app, KeyCode::Enter));
        assert_eq!(view.pane(), Pane::Tree);
        assert!(press(&mut view, &app, KeyCode::BackTab));
        assert_eq!(view.pane(), Pane::List);
        assert!(press(&mut view, &app, KeyCode::Tab));
        assert_eq!(view.pane(), Pane::Tree);
        // Tree: j/k move, h/l collapse and expand, Esc goes back to the list.
        assert!(press(&mut view, &app, KeyCode::Char('j')));
        assert_eq!(view.selected_path.as_deref(), Some("status"));
        assert!(press(&mut view, &app, KeyCode::Down));
        assert_eq!(view.selected_path.as_deref(), Some("serial_number"));
        assert!(press(&mut view, &app, KeyCode::Char('k')));
        assert_eq!(view.selected_path.as_deref(), Some("status"));
        select(&mut view, "cells");
        assert!(press(&mut view, &app, KeyCode::Char('h')));
        assert!(view.toggled.contains("cells"), "h collapses an open array");
        rebuild(&mut view);
        assert!(press(&mut view, &app, KeyCode::Char('l')));
        assert!(!view.toggled.contains("cells"), "l expands it again");
        rebuild(&mut view);
        select(&mut view, "cells[2].voltage");
        assert!(press(&mut view, &app, KeyCode::Left));
        assert_eq!(
            view.selected_path.as_deref(),
            Some("cells[2]"),
            "h on a leaf selects the parent"
        );
        // a arms and bypasses from the tree and the list.
        assert!(press(&mut view, &app, KeyCode::Char('a')));
        assert!(!file(&path).interception[0].armed);
        assert!(press(&mut view, &app, KeyCode::Esc));
        assert_eq!(view.pane(), Pane::List);
        assert!(press(&mut view, &app, KeyCode::Char('a')));
        assert!(file(&path).interception[0].armed);
        // A key the tab does not bind is left to the global bindings.
        assert!(!press(&mut view, &app, KeyCode::Char('q')));
        assert!(!press(&mut view, &app, KeyCode::Char('3')));
        // In the editor, the focused pane takes precedence: q and 3 are text.
        select(&mut view, "status");
        view.focus = Some(Pane::Tree);
        assert!(press(&mut view, &app, KeyCode::Enter));
        assert_eq!(view.pane(), Pane::Editor);
        assert!(press(&mut view, &app, KeyCode::Char('3')));
        assert!(press(&mut view, &app, KeyCode::Esc));
        assert_eq!(view.pane(), Pane::Tree);
        assert!(view.edit.is_none(), "Esc cancels the edit");
    }

    #[test]
    fn enter_in_editor_writes_a_value_override_and_bumps_revision() {
        let (mut view, app, path) = bench("value");
        view.focus = Some(Pane::Tree);
        select(&mut view, "cells[2].voltage");
        press(&mut view, &app, KeyCode::Enter);
        let edit = view.edit.clone().expect("an edit opens");
        assert_eq!(edit.text, "3.5", "it starts from the input value");
        for _ in 0..3 {
            press(&mut view, &app, KeyCode::Backspace);
        }
        type_text(&mut view, &app, "0.0");
        press(&mut view, &app, KeyCode::Enter);
        let config = file(&path);
        assert_eq!(config.revision, 4);
        assert_eq!(
            config.interception[0].overrides["cells[2].voltage"],
            OverrideSpec::Value { value: "0.0".to_owned() }
        );
        assert!(view.status(Instant::now()).pending);
        view.on_state(&state_doc(&path, 4), Instant::now());
        assert!(!view.status(Instant::now()).pending);
    }

    #[test]
    fn space_cycles_keep_set_clear_and_writes_bits() {
        let (mut view, app, path) = bench("bits");
        view.focus = Some(Pane::Tree);
        select(&mut view, "status");
        press(&mut view, &app, KeyCode::Enter);
        assert_eq!(view.edit.as_ref().unwrap().bits.len(), 16);
        press(&mut view, &app, KeyCode::Left);
        press(&mut view, &app, KeyCode::Char(' '));
        press(&mut view, &app, KeyCode::Left);
        press(&mut view, &app, KeyCode::Left);
        press(&mut view, &app, KeyCode::Char(' '));
        press(&mut view, &app, KeyCode::Char(' '));
        assert_eq!(
            file(&path).interception[0].overrides["status"],
            OverrideSpec::Bits { set: "0x2".to_owned(), clear: "0x8".to_owned() }
        );
        press(&mut view, &app, KeyCode::Char(' '));
        press(&mut view, &app, KeyCode::Right);
        press(&mut view, &app, KeyCode::Right);
        press(&mut view, &app, KeyCode::Char(' '));
        press(&mut view, &app, KeyCode::Char(' '));
        assert!(file(&path).interception[0].overrides.is_empty(), "all keep removes the override");
    }

    #[test]
    fn x_clears_field_and_shift_x_clears_all() {
        let (mut view, app, path) = bench("clear");
        view.set_override("status", OverrideSpec::Value { value: "1".to_owned() });
        view.set_override("serial_number", OverrideSpec::Value { value: "Z".to_owned() });
        view.focus = Some(Pane::Tree);
        select(&mut view, "status");
        press(&mut view, &app, KeyCode::Char('x'));
        assert_eq!(file(&path).interception[0].overrides.len(), 1);
        press(&mut view, &app, KeyCode::Char('X'));
        assert!(file(&path).interception[0].overrides.is_empty());
        view.set_override("status", OverrideSpec::Value { value: "1".to_owned() });
        press(&mut view, &app, KeyCode::Enter);
        assert!(view.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL, &app.topics, None));
        assert!(file(&path).interception[0].overrides.is_empty(), "Ctrl-x clears in the editor");
    }

    #[test]
    fn delete_asks_and_only_y_confirms() {
        // Delete from the fields asks; Esc cancels.
        let (mut view, app, path) = bench("delete_key");
        view.focus = Some(Pane::Tree);
        press(&mut view, &app, KeyCode::Delete);
        assert!(view.confirm_delete);
        press(&mut view, &app, KeyCode::Esc);
        assert_eq!(file(&path).interception.len(), 1);
        // d asks too; n keeps it, and the key after d only answers the question.
        press(&mut view, &app, KeyCode::Char('d'));
        press(&mut view, &app, KeyCode::Char('n'));
        assert_eq!(file(&path).interception.len(), 1);
        assert!(view.form.is_none(), "the key after d only answers the question");
        // d then y deletes.
        press(&mut view, &app, KeyCode::Char('d'));
        press(&mut view, &app, KeyCode::Char('y'));
        let found = file(&path).interception;
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn new_interception_form() {
        let (mut view, app, path) = bench("new");
        press(&mut view, &app, KeyCode::Char('n'));
        let form = view.form.clone().expect("form");
        assert_eq!(form.input, "/picked", "input defaults to the topic picked in Topics");
        assert_eq!(form.output, "/picked/intercepted");
        assert_eq!(form.name, "picked", "the name follows the input");
        press(&mut view, &app, KeyCode::BackTab);
        assert_eq!(view.form.as_ref().unwrap().field, FormField::Name);
        type_text(&mut view, &app, "Bad");
        press(&mut view, &app, KeyCode::Enter);
        assert!(
            !view.form.as_ref().expect("still open").error.is_empty(),
            "the form shows no error"
        );
        for _ in 0.."pickedBad".len() {
            press(&mut view, &app, KeyCode::Backspace);
        }
        type_text(&mut view, &app, "lidar");
        press(&mut view, &app, KeyCode::Enter);
        assert!(view.form.is_none());
        let config = file(&path);
        assert_eq!(config.interception.len(), 2);
        assert_eq!(config.interception[1].input, "/picked");
        assert_eq!(view.selected.as_deref(), Some("lidar"));
    }

    #[test]
    fn topic_hints_filter_pick_and_fill_the_other_fields() {
        let (mut view, mut app, path) = bench("hints");
        app.on_sample("/lidar/points", 2, &[0]);
        app.on_sample("/battery/raw/extra", 3, &[0]);
        app.on_sample("/hypha/intercept/bench/state", 4, &[0]);
        press(&mut view, &app, KeyCode::Char('n'));
        for _ in 0.."/picked".len() {
            press(&mut view, &app, KeyCode::Backspace);
        }
        let form = view.form.clone().unwrap();
        assert_eq!((form.output.as_str(), form.name.as_str()), ("", ""));
        let all = suggestions(&form, &app.topics);
        assert!(all.iter().all(|(topic, _)| !is_state_topic(topic)), "{all:?}");
        type_text(&mut view, &app, "RAW");
        let hits = suggestions(view.form.as_ref().unwrap(), &app.topics);
        let names: Vec<&str> = hits.iter().map(|(topic, _)| *topic).collect();
        assert_eq!(names, ["/battery/raw", "/battery/raw/extra"], "case-insensitive substring");
        type_text(&mut view, &app, "x");
        let found = suggestions(view.form.as_ref().unwrap(), &app.topics);
        assert!(found.is_empty(), "{found:?}");
        press(&mut view, &app, KeyCode::Backspace);
        press(&mut view, &app, KeyCode::Down);
        press(&mut view, &app, KeyCode::Down);
        press(&mut view, &app, KeyCode::Down);
        assert_eq!(view.form.as_ref().unwrap().pick, Some(1), "the pick stops at the last hint");
        press(&mut view, &app, KeyCode::Up);
        press(&mut view, &app, KeyCode::Enter);
        let form = view.form.clone().expect("Enter on a hint fills the field, it does not submit");
        assert_eq!(form.input, "/battery/raw");
        assert_eq!(form.output, "/battery/raw/intercepted");
        assert_eq!(form.name, "battery_raw");
        press(&mut view, &app, KeyCode::Tab);
        type_text(&mut view, &app, "2");
        press(&mut view, &app, KeyCode::Tab);
        press(&mut view, &app, KeyCode::BackTab);
        press(&mut view, &app, KeyCode::BackTab);
        type_text(&mut view, &app, "/x");
        let form = view.form.clone().unwrap();
        assert_eq!(form.output, "/battery/raw/intercepted2", "a typed output stays");
        assert_eq!(form.name, "battery_raw_x", "an untyped name keeps following");
        press(&mut view, &app, KeyCode::Esc);
        assert!(view.form.is_none());
        assert_eq!(file(&path).interception.len(), 1, "Esc adds nothing");
    }

    #[test]
    fn names_from_topics() {
        assert_eq!(name_from_topic("/battery/raw"), "battery_raw");
        assert_eq!(name_from_topic("/Uav1//GPS-fix/"), "uav1_gps_fix");
        assert_eq!(name_from_topic(""), "");
    }

    #[test]
    fn pending_until_backend_echoes_revision_and_offline_after_two_seconds() {
        let (mut view, app, path) = bench("pending");
        let start = Instant::now();
        assert!(!view.status(start).pending);
        press(&mut view, &app, KeyCode::Char('a'));
        assert!(view.status(start).pending);
        assert_eq!(view.status(start).armed, Some(false));
        view.on_state(&state_doc(&path, 4), start);
        assert!(!view.status(start).pending);
        assert!(!view.status(start).offline);
        assert!(view.status(start + OFFLINE_AFTER).offline);
        assert!(InterceptView::default().status(start).offline, "never seen is offline");
    }

    #[test]
    fn invalid_value_stays_in_buffer_with_error() {
        let (mut view, app, path) = bench("invalid");
        view.focus = Some(Pane::Tree);
        select(&mut view, "status");
        press(&mut view, &app, KeyCode::Enter);
        view.edit.as_mut().unwrap().text.clear();
        view.edit.as_mut().unwrap().cursor = 0;
        type_text(&mut view, &app, "70000");
        press(&mut view, &app, KeyCode::Enter);
        let edit = view.edit.as_ref().unwrap();
        assert_eq!(edit.text, "70000");
        assert!(edit.error.contains("uint16"), "{}", edit.error);
        assert!(file(&path).interception[0].overrides.is_empty());
        assert_eq!(file(&path).revision, 3, "nothing was written");
    }

    #[test]
    fn a_hand_broken_file_is_not_overwritten() {
        let (mut view, app, path) = bench("broken");
        std::fs::write(&path, "revision = [").unwrap();
        press(&mut view, &app, KeyCode::Char('a'));
        assert!(!view.status(Instant::now()).message.is_empty(), "the status line is empty");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "revision = [");
    }

    #[test]
    fn narrow_layout_tab_goes_deeper_and_esc_back() {
        let (mut view, app, _path) = bench("narrow");
        assert_eq!(layout_for(70, 20), Layout::Narrow);
        select(&mut view, "status");
        press(&mut view, &app, KeyCode::Tab);
        assert_eq!(view.pane(), Pane::Tree);
        press(&mut view, &app, KeyCode::Tab);
        assert_eq!(view.pane(), Pane::Editor);
        press(&mut view, &app, KeyCode::Esc);
        assert_eq!(view.pane(), Pane::Tree);
        press(&mut view, &app, KeyCode::Esc);
        assert_eq!(view.pane(), Pane::List);
        assert_eq!(layout_for(59, 30), Layout::TooSmall);
        assert_eq!(layout_for(100, 11), Layout::TooSmall);
        assert_eq!(layout_for(109, 30), Layout::Medium);
        assert_eq!(layout_for(110, 30), Layout::Wide);
    }

    #[test]
    fn state_topics_are_recognized() {
        assert!(is_state_topic("/hypha/intercept/bench/state"));
        assert!(!is_state_topic("/hypha/intercept//state"));
        assert!(!is_state_topic("/bench/state"));
        let mut view = InterceptView::default();
        view.on_state(b"\x03\0\0\0abc", Instant::now());
        assert_eq!(view.bad_state_docs, 1);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod guard_tests {
    use super::*;

    /// A fresh root under the system temp directory, canonical, holding the guarded
    /// `inside` directory (with a `sub` directory) and a sibling `outside` directory.
    fn fresh_root(test: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("hypha_top_guard_{test}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("inside").join("sub")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn a_config_path_outside_the_directory_is_refused() {
        let root = fresh_root("outside");
        let dir = root.join("inside");
        let outside = root.join("outside");
        std::fs::write(outside.join("x.toml"), "").unwrap();
        // A file in another directory, and one reached by `..` out of the directory.
        assert!(confined_path(&dir, &outside.join("x.toml")).is_none());
        assert!(confined_path(&dir, &dir.join("..").join("outside").join("x.toml")).is_none());
        assert!(
            confined_path(
                &dir,
                &dir.join("sub").join("..").join("..").join("outside").join("x.toml")
            )
            .is_none()
        );
        // A name that only shares a prefix with the directory is another directory.
        let sibling = root.join("inside_sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(confined_path(&dir, &sibling.join("x.toml")).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_config_path_inside_the_directory_is_kept() {
        let root = fresh_root("inside");
        let dir = root.join("inside");
        let inside = dir.join("sub").join("x.toml");
        assert_eq!(confined_path(&dir, &inside), Some(inside.clone()));
        // A new file under the directory is accepted before it exists.
        let new_file = dir.join("new.toml");
        assert_eq!(confined_path(&dir, &new_file), Some(new_file.clone()));
        // `..` that comes back inside is resolved to the inside path.
        let back = dir.join("sub").join("..").join("x.toml");
        assert_eq!(confined_path(&dir, &back), Some(dir.join("x.toml")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_relative_path_is_refused() {
        let root = fresh_root("relative");
        let dir = root.join("inside");
        assert!(confined_path(&dir, Path::new("intercept.toml")).is_none());
        assert!(confined_path(&dir, Path::new("./intercept.toml")).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_symlink_out_of_the_directory_is_refused() {
        let root = fresh_root("symlink");
        let dir = root.join("inside");
        let target = root.join("outside").join("secret.toml");
        std::fs::write(&target, "").unwrap();
        let link = dir.join("sub").join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(confined_path(&dir, &link).is_none(), "a link out of the directory is refused");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_symlink_inside_the_directory_resolves_to_its_target() {
        let root = fresh_root("symlink_inside");
        let dir = root.join("inside");
        let target = dir.join("sub").join("real.toml");
        std::fs::write(&target, "").unwrap();
        let link = dir.join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(confined_path(&dir, &link), Some(target));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_tab_refuses_a_state_document_that_names_a_path_outside_the_directory() {
        let root = fresh_root("on_state");
        let dir = root.join("inside");
        let outside = root.join("outside").join("intercept.toml");
        std::fs::write(&outside, "revision = 42\n").unwrap();
        let mut view = InterceptView { intercept_dir: Some(dir), ..InterceptView::default() };
        view.on_state(&crate::top::intercept::tests::state_doc(&outside, 1), Instant::now());

        assert_eq!(view.config.revision, 0, "the outside file is not read");
        assert!(view.message.contains("refused"), "{}", view.message);
        assert!(view.backend().is_some(), "the backend is still shown");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_tab_write_is_refused_outside_the_directory() {
        let root = fresh_root("write");
        let dir = root.join("inside");
        let outside = root.join("outside").join("intercept.toml");
        let before = "revision = 7\n";
        std::fs::write(&outside, before).unwrap();
        let mut view = InterceptView { intercept_dir: Some(dir), ..InterceptView::default() };
        view.on_state(&crate::top::intercept::tests::state_doc(&outside, 7), Instant::now());
        view.mutate(|_config| Ok(()));

        assert!(view.message.starts_with("refused:"), "{}", view.message);
        assert!(view.message.contains("is outside"), "{}", view.message);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), before, "the file is unchanged");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
