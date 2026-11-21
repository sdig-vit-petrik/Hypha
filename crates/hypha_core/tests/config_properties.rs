//! Property tests of the TOML parameter loader (`Parameters::from_toml_file`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use hypha_core::parameters::{ListElement, ParamValue, Parameters};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use toml::{Table, Value};

/// The loader's limit on included-file reads per composition (`parameters_load.rs`).
const MAX_INCLUDED_FILES: usize = 256;

/// Shrunk failures are kept here, relative to the crate directory, and replayed first.
const REGRESSIONS: &str = "proptest-regressions/config_properties.txt";

/// Miri interprets every step: a handful of cases still walks each code path.
const MIRI_CASES: u32 = 4;

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases: if cfg!(miri) { MIRI_CASES } else { cases },
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(REGRESSIONS))),
        ..ProptestConfig::default()
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A fresh directory under the system temp directory, removed when dropped.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, TestCaseError> {
        let dir = std::env::temp_dir().join(format!(
            "hypha_config_properties_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|error| TestCaseError::fail(error.to_string()))?;
        Ok(Self { dir })
    }

    fn write(&self, name: &str, text: &str) -> Result<PathBuf, TestCaseError> {
        let path = self.dir.join(name);
        std::fs::write(&path, text).map_err(|error| TestCaseError::fail(error.to_string()))?;
        Ok(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Cleanup must not panic while a failing case unwinds; a leftover is harmless.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A scalar TOML value the loader keeps: no date-time, no NaN, no `${` in text.
fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Boolean),
        any::<i64>().prop_map(Value::Integer),
        any::<f64>().prop_filter("NaN", |value| !value.is_nan()).prop_map(Value::Float),
        "\\PC{0,12}"
            .prop_filter("a reference", |text| !text.contains("${"))
            .prop_map(Value::String),
    ]
}

/// A key that is not reserved and needs no quoting.
fn key() -> impl Strategy<Value = String> {
    "k[a-z0-9_]{0,4}"
}

/// A table of scalars, lists and tables, a few levels deep.
fn table() -> impl Strategy<Value = Table> {
    let element_table =
        btree_map(key(), scalar(), 0..4).prop_map(|map| map.into_iter().collect::<Table>());
    let leaf = prop_oneof![
        4 => scalar(),
        1 => vec(scalar(), 0..4).prop_map(Value::Array),
        1 => vec(element_table.prop_map(Value::Table), 1..3).prop_map(Value::Array),
    ];
    let tree = leaf.prop_recursive(3, 24, 4, |inner| {
        btree_map(key(), inner, 0..4).prop_map(|map| Value::Table(map.into_iter().collect()))
    });
    btree_map(key(), tree, 0..6).prop_map(|map| map.into_iter().collect())
}

fn param(value: &Value) -> Option<ParamValue> {
    match value {
        Value::Boolean(flag) => Some(ParamValue::Bool(*flag)),
        Value::Integer(number) => Some(ParamValue::Int(*number)),
        Value::Float(number) => Some(ParamValue::Float(*number)),
        Value::String(text) => Some(ParamValue::Text(text.clone())),
        Value::Datetime(_) | Value::Array(_) | Value::Table(_) => None,
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") }
}

/// Checks that `params` holds `table` under `prefix`: each scalar by its dotted
/// name, each list by its name and elements.
fn check_holds(params: &Parameters, table: &Table, prefix: &str) -> Result<(), TestCaseError> {
    for (name, value) in table {
        let dotted = join(prefix, name);
        match value {
            Value::Table(nested) => check_holds(params, nested, &dotted)?,
            Value::Array(items) => {
                prop_assert_eq!(params.list_len(&dotted), Some(items.len()), "list {}", dotted);
                for (index, item) in items.iter().enumerate() {
                    let element = params.list_element(&dotted, index);
                    match (item, element) {
                        (Value::Table(fields), Some(ListElement::Table(loaded))) => {
                            check_holds(loaded, fields, "")?;
                            prop_assert_eq!(loaded.names().len(), fields.len());
                        }
                        (scalar, Some(ListElement::Scalar(loaded))) => {
                            let expected = param(scalar);
                            prop_assert_eq!(
                                Some(loaded),
                                expected.as_ref(),
                                "{}[{}]",
                                dotted,
                                index
                            );
                        }
                        (_, other) => {
                            return Err(TestCaseError::fail(format!(
                                "{dotted}[{index}] is {other:?}"
                            )));
                        }
                    }
                }
            }
            scalar => {
                let expected = param(scalar);
                prop_assert_eq!(params.value(&dotted), expected.as_ref(), "scalar {}", dotted);
            }
        }
    }
    Ok(())
}

fn scalar_count(table: &Table) -> usize {
    table
        .values()
        .map(|value| match value {
            Value::Table(nested) => scalar_count(nested),
            Value::Array(_) => 0,
            _ => 1,
        })
        .fold(0, usize::saturating_add)
}

/// Variables this file sets, one per index; nothing else sets them.
fn variable(index: usize) -> String {
    format!("HYPHA_CONFIG_PROPERTY_V{index}")
}

const VARIABLES: usize = 4;

/// One piece of a text value: literal text or a `${NAME}` reference.
#[derive(Debug, Clone)]
enum Piece {
    Literal(String),
    Reference(usize),
}

/// A reference the loader must refuse, with what is wrong with it.
fn bad_reference() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::from("${HYPHA_CONFIG_PROPERTY_UNSET}")),
        Just(String::from("${}")),
        "[0-9-][A-Z]{0,4}".prop_map(|name| format!("${{{name}}}")),
        "[A-Z]{1,4}[-.$ ][A-Z]{0,4}".prop_map(|name| format!("${{{name}}}")),
        "[A-Z_]{0,6}".prop_map(|name| format!("${{{name}")),
    ]
}

/// One file of an include graph: the files it includes, by index, and whether it
/// writes a key of its own.
#[derive(Debug, Clone)]
struct GraphFile {
    includes: Vec<usize>,
    has_key: bool,
    has_table: bool,
}

fn graph() -> impl Strategy<Value = Vec<GraphFile>> {
    vec(
        (vec(0..6_usize, 0..4), any::<bool>(), any::<bool>())
            .prop_map(|(includes, has_key, has_table)| GraphFile { includes, has_key, has_table }),
        1..6,
    )
}

fn graph_text(files: &[GraphFile], index: usize) -> String {
    let Some(file) = files.get(index) else {
        return String::new();
    };
    let count = files.len();
    let names: Vec<String> = file
        .includes
        .iter()
        .filter_map(|target| target.checked_rem(count))
        .map(|target| format!("\"f{target}.toml\""))
        .collect();
    let mut text = format!("include = [{}]\n", names.join(", "));
    if file.has_key {
        let _ = writeln!(text, "key_{index} = {index}");
    }
    if file.has_table {
        text.push_str("[shared]\n");
    }
    text
}

/// What the loader should give the graph from `f0`: `Ok` with the keys of every file it
/// reaches, or `Err` for a cycle or a key written twice.
fn graph_outcome(files: &[GraphFile]) -> Result<Vec<usize>, ()> {
    fn visit(
        files: &[GraphFile],
        index: usize,
        path: &mut Vec<usize>,
        visits: &mut BTreeMap<usize, usize>,
    ) -> Result<(), ()> {
        if path.contains(&index) {
            return Err(());
        }
        let count = visits.entry(index).or_default();
        *count = count.saturating_add(1);
        path.push(index);
        let includes = files.get(index).map_or(&[][..], |file| file.includes.as_slice());
        for target in includes {
            let target = target.checked_rem(files.len()).ok_or(())?;
            visit(files, target, path, visits)?;
        }
        path.pop();
        Ok(())
    }
    let mut visits = BTreeMap::new();
    visit(files, 0, &mut Vec::new(), &mut visits)?;
    let keyed_twice = visits
        .iter()
        .any(|(index, count)| *count > 1 && files.get(*index).is_some_and(|file| file.has_key));
    if keyed_twice {
        return Err(());
    }
    Ok(visits
        .keys()
        .copied()
        .filter(|index| files.get(*index).is_some_and(|file| file.has_key))
        .collect())
}

/// TOML-ish text from the pieces the loader cares about.
fn toml_soup() -> impl Strategy<Value = String> {
    let token = prop_oneof![
        Just("\n"),
        Just("[a]"),
        Just("[[a]]"),
        Just("[log]"),
        Just("[log.sink.s]"),
        Just("include = "),
        Just("[\"root.toml\"]"),
        Just("[\"other.toml\"]"),
        Just("[]"),
        Just("a = "),
        Just("b.c = "),
        Just("path = "),
        Just("\"rel/x\""),
        Just("\"${HOME}\""),
        Just("\"${\""),
        Just("\"${A\""),
        Just("1"),
        Just("1.5"),
        Just("nan"),
        Just("true"),
        Just("1979-05-27T07:32:00Z"),
        Just("[1, 2]"),
        Just("[[1], [2]]"),
        Just("{ x = 1 }"),
        Just("[{ x = [1] }]"),
        Just("node = "),
        Just("ports = "),
        Just(","),
        Just("\""),
    ];
    vec(token, 0..24).prop_map(|tokens| tokens.concat())
}

fn load(path: &Path) -> Result<Parameters, String> {
    Parameters::from_toml_file(path).map_err(|error| error.to_string())
}

proptest! {
    #![proptest_config(config(256))]

    #[test]
    fn a_generated_table_loads_to_its_scalars_and_lists(table in table()) {
        let fixture = Fixture::new()?;
        let text = toml::to_string(&table).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let path = fixture.write("root.toml", &text)?;
        let params = load(&path).map_err(|error| TestCaseError::fail(format!("{error}\n{text}")))?;
        check_holds(&params, &table, "")?;
        prop_assert_eq!(params.names().len(), scalar_count(&table));
    }

    #[test]
    fn a_reference_expands_once_to_its_variable(
        values in vec("\\PC{1,12}", VARIABLES),
        pieces in vec(prop_oneof![
            "[a-zA-Z0-9 _./#=é-]{0,6}".prop_map(Piece::Literal),
            (0..VARIABLES).prop_map(Piece::Reference),
        ], 0..8),
        in_include in any::<bool>(),
    ) {
        let fixture = Fixture::new()?;
        for (index, value) in values.iter().enumerate() {
            hypha_testkit::set_env(variable(index), value);
        }
        let mut written = String::new();
        let mut expected = String::new();
        for piece in &pieces {
            match piece {
                Piece::Literal(text) => {
                    written.push_str(text);
                    expected.push_str(text);
                }
                Piece::Reference(index) => {
                    let _ = write!(written, "${{{}}}", variable(*index));
                    expected.push_str(values.get(*index).map_or("", String::as_str));
                }
            }
        }
        let root = if in_include {
            // An expansion with a `/` would name a file outside the fixture.
            prop_assume!(!expected.contains('/'));
            fixture.write(&format!("{expected}.toml"), "x = \"y\"\n")?;
            let entry = Value::String(format!("{written}.toml"));
            let table = Table::from_iter([(String::from("include"), Value::Array(vec![entry]))]);
            let text = toml::to_string(&table).map_err(|error| TestCaseError::fail(error.to_string()))?;
            fixture.write("root.toml", &text)?
        } else {
            let table = Table::from_iter([(String::from("value"), Value::String(written.clone()))]);
            let text = toml::to_string(&table).map_err(|error| TestCaseError::fail(error.to_string()))?;
            fixture.write("root.toml", &text)?
        };
        let loaded = load(&root);
        if in_include {
            let params = loaded.map_err(|error| TestCaseError::fail(format!("{error}: {written:?}")))?;
            prop_assert_eq!(params.value("x"), Some(&ParamValue::Text(String::from("y"))));
        } else {
            let params = loaded.map_err(|error| TestCaseError::fail(format!("{error}: {written:?}")))?;
            prop_assert_eq!(params.value("value"), Some(&ParamValue::Text(expected)));
        }
    }

    #[test]
    fn a_bad_reference_is_refused_naming_its_key(
        before in "[a-z ]{0,4}",
        bad in bad_reference(),
        after in "[a-z ]{0,4}",
    ) {
        let fixture = Fixture::new()?;
        let table = Table::from_iter([(String::from("t"), Value::Table(Table::from_iter([(String::from("value"), Value::String(format!("{before}{bad}{after}")))])))]);
        let text = toml::to_string(&table).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let path = fixture.write("root.toml", &text)?;
        let error = Parameters::from_toml_file(&path)
            .map(|_| ())
            .err()
            .ok_or_else(|| TestCaseError::fail(format!("{bad:?} was accepted")))?;
        prop_assert_eq!(error.key.as_str(), "t.value");
    }

    #[test]
    fn an_include_graph_loads_its_keys_or_is_refused(files in graph()) {
        let fixture = Fixture::new()?;
        for index in 0..files.len() {
            fixture.write(&format!("f{index}.toml"), &graph_text(&files, index))?;
        }
        let loaded = load(&fixture.dir.join("f0.toml"));
        match graph_outcome(&files) {
            Ok(keys) => {
                let params = loaded.map_err(|error| TestCaseError::fail(format!("{error}\n{files:?}")))?;
                for index in &keys {
                    let expected = i64::try_from(*index).map_err(|error| TestCaseError::fail(error.to_string()))?;
                    prop_assert_eq!(params.value(&format!("key_{index}")), Some(&ParamValue::Int(expected)));
                }
                prop_assert_eq!(params.names().len(), keys.len());
            }
            Err(()) => prop_assert!(loaded.is_err(), "{:?} loaded", files),
        }
    }

    #[test]
    fn any_text_is_loaded_or_refused_without_a_panic(
        text in prop_oneof![toml_soup(), "\\PC{0,48}", any::<String>()],
        other in toml_soup(),
    ) {
        let fixture = Fixture::new()?;
        fixture.write("other.toml", &other)?;
        let path = fixture.write("root.toml", &text)?;
        let _ = Parameters::from_toml_file(&path);
    }
}

proptest! {
    #![proptest_config(config(32))]

    #[test]
    #[cfg_attr(miri, ignore = "every case reads up to 256 files, slow under an interpreter")]
    fn a_wide_include_tree_of_shared_files_finishes(
        depth in 1..=16_usize,
        fan in 1..=4_usize,
    ) {
        // Each file includes the next `fan` times; no file writes a key.
        let fixture = Fixture::new()?;
        for level in 0..depth {
            let next = format!("\"n{}.toml\"", level.saturating_add(1));
            let includes = vec![next; fan].join(", ");
            fixture.write(&format!("n{level}.toml"), &format!("include = [{includes}]\n[shared]\n"))?;
        }
        fixture.write(&format!("n{depth}.toml"), "[shared]\n")?;
        let loaded = load(&fixture.dir.join("n0.toml"));
        // Every level reads `fan` times the files of the level above it.
        let reads = (1..=depth)
            .scan(1_usize, |files, _| {
                *files = files.saturating_mul(fan);
                Some(*files)
            })
            .fold(0_usize, usize::saturating_add);
        if reads <= MAX_INCLUDED_FILES {
            prop_assert!(loaded.is_ok(), "{} reads: {:?}", reads, loaded);
        } else {
            let error = loaded.err().unwrap_or_default();
            prop_assert!(error.contains("includes read more than 256 files"), "{} reads: {}", reads, error);
        }
    }
}
