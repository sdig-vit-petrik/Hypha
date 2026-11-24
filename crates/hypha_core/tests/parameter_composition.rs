//! Config-file composition: a top-level `include` key merges other TOML files.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use hypha_core::parameters::{ParamValue, ParameterLoadError, Parameters};

/// A directory under the system temp directory. It is removed when the test ends.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("hypha_composition_{}_{label}", std::process::id()));
        // A fixture left by an earlier run may be absent; that is the normal case.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture directory");
        Self { dir }
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture subdirectory");
        }
        std::fs::write(&path, text).expect("write fixture file");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Cleanup must not panic while a failing test is already unwinding.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn load(path: &Path) -> Parameters {
    Parameters::from_toml_file(path).expect("load composed parameters")
}

fn refused(path: &Path) -> ParameterLoadError {
    Parameters::from_toml_file(path).expect_err("composition must be refused")
}

/// Write `chain_0.toml` to `chain_<count>.toml`, each including the next.
fn write_chain(fixture: &Fixture, count: usize) -> PathBuf {
    for (index, next) in (0..count).zip(1..) {
        fixture
            .write(&format!("chain_{index}.toml"), &format!("include = [\"chain_{next}.toml\"]\n"));
    }
    fixture.write(&format!("chain_{count}.toml"), "leaf = true\n");
    fixture.dir.join("chain_0.toml")
}

#[test]
fn keys_from_included_files_are_visible() {
    let fixture = Fixture::new("flat");
    fixture.write("a.toml", "from_a = 1\n");
    fixture.write("b.toml", "from_b = \"b\"\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\", \"b.toml\"]\n");
    let params = load(&root);
    assert_eq!(params.value("from_a"), Some(&ParamValue::Int(1)));
    assert_eq!(params.value("from_b"), Some(&ParamValue::Text("b".into())));
}

#[test]
fn a_key_in_two_includes_is_still_refused() {
    let fixture = Fixture::new("override");
    fixture.write("a.toml", "shared = \"a\"\norder = \"a\"\n");
    fixture.write("b.toml", "order = \"b\"\n");
    let siblings = fixture.write("siblings.toml", "include = [\"a.toml\", \"b.toml\"]\n");
    let error = refused(&siblings);
    assert_eq!(error.key, "order");
    assert!(error.reason.contains("conflict"), "reason: {}", error.reason);
}

#[test]
fn a_root_scalar_overrides_the_same_key_of_an_include() {
    let fixture = Fixture::new("root_overrides");
    fixture.write("a.toml", "shared = \"a\"\nkept = 1\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\nshared = \"root\"\n");
    let params = load(&root);
    assert_eq!(params.value("shared"), Some(&ParamValue::Text("root".into())));
    assert_eq!(params.value("kept"), Some(&ParamValue::Int(1)));
}

#[test]
fn a_root_scalar_overrides_a_key_of_a_nested_include() {
    let fixture = Fixture::new("root_overrides_nested");
    fixture.write("deep/c.toml", "rate_hz = 50\n");
    fixture.write("b.toml", "include = [\"deep/c.toml\"]\n");
    let root = fixture.write("root.toml", "include = [\"b.toml\"]\nrate_hz = 20\n");
    let params = load(&root);
    assert_eq!(params.value("rate_hz"), Some(&ParamValue::Int(20)));
    assert_eq!(params.origin_of("rate_hz"), Some(fixture.dir.as_path()));
}

#[test]
fn an_intermediate_include_does_not_override_its_own_includes() {
    let fixture = Fixture::new("intermediate_refused");
    fixture.write("c.toml", "rate_hz = 50\n");
    fixture.write("b.toml", "include = [\"c.toml\"]\nrate_hz = 30\n");
    let root = fixture.write("root.toml", "include = [\"b.toml\"]\n");
    let error = refused(&root);
    assert_eq!(error.key, "rate_hz");
    assert!(error.reason.contains("conflict"), "reason: {}", error.reason);
}

#[test]
fn a_root_table_merges_with_an_include_table_and_wins_on_shared_keys() {
    let fixture = Fixture::new("root_table_merge");
    fixture.write("a.toml", "[drive]\nspeed = 2\nlimit = 9\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\n[drive]\nspeed = 3\n");
    let params = load(&root);
    assert_eq!(params.value("drive.speed"), Some(&ParamValue::Int(3)));
    assert_eq!(params.value("drive.limit"), Some(&ParamValue::Int(9)));
}

#[test]
fn a_root_scalar_over_an_include_table_is_refused() {
    let fixture = Fixture::new("root_kind_conflict");
    fixture.write("a.toml", "[drive]\nspeed = 2\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\ndrive = 1\n");
    let error = refused(&root);
    assert!(error.path.ends_with("root.toml"), "{}", error.path);
    assert_eq!(error.key, "drive");
    assert_eq!(error.reason, "conflicts with an included file");
    let list = fixture.write("list.toml", "include = [\"a.toml\"]\n[[drive]]\nspeed = 1\n");
    assert_eq!(refused(&list).key, "drive");
}

#[test]
fn an_override_is_listed_with_both_files() {
    let fixture = Fixture::new("override_listed");
    let included = fixture.write("a.toml", "shared = \"a\"\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\nshared = \"root\"\n");
    let params = load(&root);
    let overrides = params.overrides();
    assert_eq!(overrides.len(), 1, "{overrides:?}");
    let first = overrides.first().expect("one override");
    assert_eq!(first.key, "shared");
    assert_eq!(first.winner, root);
    assert_eq!(first.overridden, included);
}

#[test]
fn tables_merge_deeply() {
    let fixture = Fixture::new("tables");
    fixture.write("a.toml", "[drive]\nspeed = 2\n[drive.limits]\nmax = 9\n");
    fixture.write("b.toml", "[drive]\nmode = \"safe\"\n[drive.limits]\nmin = 1\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\", \"b.toml\"]\n");
    let params = load(&root);
    assert_eq!(params.value("drive.speed"), Some(&ParamValue::Int(2)));
    assert_eq!(params.value("drive.mode"), Some(&ParamValue::Text("safe".into())));
    assert_eq!(params.value("drive.limits.max"), Some(&ParamValue::Int(9)));
    assert_eq!(params.value("drive.limits.min"), Some(&ParamValue::Int(1)));
}

#[test]
fn a_scalar_and_a_table_with_the_same_name_across_includes_are_refused() {
    let fixture = Fixture::new("scalar_over_table");
    fixture.write("a.toml", "[sensor]\ngain = 1\n");
    fixture.write("b.toml", "sensor = 2.5\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\", \"b.toml\"]\n");
    let error = refused(&root);
    assert_eq!(error.key, "sensor");
    assert!(error.reason.contains("conflict"), "reason: {}", error.reason);
}

#[test]
fn a_file_included_by_two_branches_is_not_a_cycle_but_refuses_duplicate_keys() {
    let fixture = Fixture::new("diamond");
    fixture.write("c.toml", "leaf = 7\n");
    fixture.write("a.toml", "include = [\"c.toml\"]\n");
    fixture.write("b.toml", "include = [\"c.toml\"]\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\", \"b.toml\"]\n");
    let error = refused(&root);
    assert_eq!(error.key, "leaf");
    assert!(error.reason.contains("conflict"), "reason: {}", error.reason);
}

#[test]
fn nested_includes_are_resolved_and_merged() {
    let fixture = Fixture::new("nested");
    fixture.write("b.toml", "deep = true\n");
    fixture.write("a.toml", "include = [\"b.toml\"]\nlevel = \"a\"\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\n");
    let params = load(&root);
    assert_eq!(params.value("deep"), Some(&ParamValue::Bool(true)));
    assert_eq!(params.value("level"), Some(&ParamValue::Text("a".into())));
}

#[test]
fn a_nested_include_resolves_against_its_own_directory() {
    let fixture = Fixture::new("relative");
    fixture.write("b.toml", "wrong = true\n");
    fixture.write("sub/b.toml", "sub_key = 3\n");
    fixture.write("sub/a.toml", "include = [\"b.toml\"]\n");
    let root = fixture.write("root.toml", "include = [\"sub/a.toml\"]\n");
    let params = load(&root);
    assert_eq!(params.value("sub_key"), Some(&ParamValue::Int(3)));
    assert!(!params.has("wrong"));
}

#[test]
fn a_missing_include_is_refused_and_names_the_file() {
    let fixture = Fixture::new("missing");
    let root = fixture.write("root.toml", "include = [\"missing.toml\"]\n");
    let error = refused(&root);
    assert_eq!(error.key, "include");
    assert_eq!(error.path, root.display().to_string());
    assert!(error.reason.contains("missing.toml"), "reason: {}", error.reason);
}

#[test]
fn a_cycle_is_refused_and_names_the_file_that_closes_it() {
    let fixture = Fixture::new("cycle");
    fixture.write("a.toml", "include = [\"b.toml\"]\n");
    let b = fixture.write("b.toml", "include = [\"a.toml\"]\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\n");
    let error = refused(&root);
    assert_eq!(error.key, "include");
    assert_eq!(error.path, b.display().to_string());
    assert!(error.reason.contains("cycle"), "reason: {}", error.reason);
}

#[test]
fn a_file_that_includes_itself_is_refused_as_a_cycle() {
    let fixture = Fixture::new("self_cycle");
    fixture.write("self.toml", "include = [\"self.toml\"]\n");
    let root = fixture.write("root.toml", "include = [\"self.toml\"]\n");
    let error = refused(&root);
    assert_eq!(error.key, "include");
    assert!(error.reason.contains("cycle"), "reason: {}", error.reason);
}

#[test]
fn a_non_array_include_is_refused() {
    let fixture = Fixture::new("not_array");
    let root = fixture.write("root.toml", "include = \"x\"\n");
    let error = refused(&root);
    assert_eq!(error.key, "include");
    assert!(error.reason.contains("array of strings"), "reason: {}", error.reason);
}

#[test]
fn an_array_of_non_strings_is_refused() {
    let fixture = Fixture::new("not_strings");
    let root = fixture.write("root.toml", "include = [1]\n");
    let error = refused(&root);
    assert_eq!(error.key, "include");
    assert!(error.reason.contains("array of strings"), "reason: {}", error.reason);
}

#[test]
fn sixteen_nested_includes_load_and_seventeen_are_refused() {
    let fixture = Fixture::new("depth_ok");
    let root = write_chain(&fixture, 16);
    assert_eq!(load(&root).value("leaf"), Some(&ParamValue::Bool(true)));

    let deep = Fixture::new("depth_too_deep");
    let root = write_chain(&deep, 17);
    let error = refused(&root);
    assert_eq!(error.key, "include");
    assert!(error.path.ends_with("chain_16.toml"), "path: {}", error.path);
    assert!(error.reason.contains("16 levels"), "reason: {}", error.reason);
}

#[test]
fn an_array_in_an_included_file_loads_as_a_list() {
    let fixture = Fixture::new("included_array");
    fixture.write("a.toml", "list = [1, 2]\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\n");
    let params = load(&root);
    assert_eq!(params.list_len("list"), Some(2));
}

#[test]
fn include_is_not_a_parameter() {
    let fixture = Fixture::new("not_parameter");
    fixture.write("a.toml", "x = 1\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\"]\n");
    let params = load(&root);
    assert!(!params.has("include"));
    assert!(params.value("include").is_none());
    assert_eq!(params.names(), vec!["x"]);
}

#[test]
fn a_reference_in_an_included_file_names_that_file() {
    let fixture = Fixture::new("env_in_include");
    fixture.write("sub/a.toml", "topic = '${HYPHA_TEST_UNSET_K2_INCLUDED}'\n");
    let root = fixture.write("root.toml", "include = ['sub/a.toml']\n");
    let error = refused(&root);
    assert!(error.path.ends_with("sub/a.toml"), "{}", error.path);
    assert_eq!(error.key, "topic");
    assert_eq!(error.reason, "environment variable `HYPHA_TEST_UNSET_K2_INCLUDED` is unset");
}

#[test]
fn an_include_path_expands_its_environment_references() {
    let fixture = Fixture::new("env_include_path");
    fixture.write("b.toml", "gain = 2\n");
    let directory = fixture.dir.to_str().expect("fixture path is text").to_owned();
    hypha_testkit::set_env("HYPHA_TEST_DIR_K2", &directory);
    let root = fixture.write("root.toml", "include = ['${HYPHA_TEST_DIR_K2}/b.toml']\n");
    let params = load(&root);
    hypha_testkit::remove_env("HYPHA_TEST_DIR_K2");
    assert_eq!(params.value("gain"), Some(&ParamValue::Int(2)));
}

#[test]
fn an_include_path_with_an_unset_variable_is_refused_at_include() {
    let fixture = Fixture::new("env_include_unset");
    let root = fixture.write("root.toml", "include = ['${HYPHA_TEST_UNSET_K2_PATH}/b.toml']\n");
    let error = refused(&root);
    assert!(error.path.ends_with("root.toml"), "{}", error.path);
    assert_eq!(error.key, "include");
    assert_eq!(error.reason, "environment variable `HYPHA_TEST_UNSET_K2_PATH` is unset");
}

#[test]
fn a_relative_value_of_an_included_file_keeps_its_directory() {
    let fixture = Fixture::new("origin_of_include");
    fixture.write("sub/a.toml", "file = 'data.txt'\n[[sensors]]\nfile = 's.bin'\n");
    let root = fixture.write("root.toml", "include = ['sub/a.toml']\nroot_file = 'r.txt'\n");
    let params = load(&root);
    let sub = fixture.dir.join("sub");
    assert_eq!(params.origin_of("file"), Some(sub.as_path()));
    assert_eq!(params.origin_of("sensors"), Some(sub.as_path()));
    assert_eq!(params.origin_of("sensors[0].file"), Some(sub.as_path()));
    assert_eq!(params.origin_of("root_file"), Some(fixture.dir.as_path()));
    assert_eq!(params.origin_of("missing"), None);
    // The value is what the file says; only the reader resolves it.
    assert_eq!(params.value("file"), Some(&ParamValue::Text("data.txt".into())));
}

#[test]
fn an_include_tree_that_reads_more_than_256_files_is_refused_at_once() {
    // 16 levels of files that each include the next four times would read 4^16 files.
    let fixture = Fixture::new("wide_tree");
    for level in 0..16_usize {
        let next = format!("\"n{}.toml\"", level.saturating_add(1));
        fixture.write(
            &format!("n{level}.toml"),
            &format!("include = [{next}, {next}, {next}, {next}]\n"),
        );
    }
    fixture.write("n16.toml", "[shared]\n");
    let error = refused(&fixture.dir.join("n0.toml"));
    assert_eq!(error.key, "include");
    assert!(error.reason.contains("more than 256 files"), "reason: {}", error.reason);
}

#[test]
fn a_file_shared_by_many_branches_within_the_file_limit_loads() {
    // Four levels of four (340 reads) are refused, three levels (84 reads) load.
    let fixture = Fixture::new("wide_tree_within");
    for level in 0..3_usize {
        let next = format!("\"n{}.toml\"", level.saturating_add(1));
        fixture.write(
            &format!("n{level}.toml"),
            &format!("include = [{next}, {next}, {next}, {next}]\n"),
        );
    }
    fixture.write("n3.toml", "[shared]\n");
    let params = load(&fixture.dir.join("n0.toml"));
    let found = params.names();
    assert!(found.is_empty(), "{found:?}");
}
