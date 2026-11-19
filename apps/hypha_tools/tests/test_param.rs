//! Tests for `hypha param` static configuration file operations.

use std::fs;
use std::process::Command;

#[test]
#[allow(clippy::unwrap_used)]
fn toml_parameter_reader_leaves_the_file_as_written() {
    let tmp_path =
        std::env::temp_dir().join(format!("test_hypha_param_{}.toml", std::process::id()));
    let initial_content = "# operator note\nfoo = \"bar\"\n\n[controller]\nbaz = 123\n";
    fs::write(&tmp_path, initial_content).unwrap();

    let str_path = tmp_path.to_str().unwrap();

    let binary = env!("CARGO_BIN_EXE_hypha");
    let dump = Command::new(binary).arg("param").args(["dump", str_path]).output().unwrap();
    assert!(dump.status.success());
    let dump_text = String::from_utf8(dump.stdout).unwrap();
    assert!(dump_text.contains("foo=bar"));
    assert!(dump_text.contains("controller.baz=123"));

    let get = Command::new(binary)
        .arg("param")
        .args(["get", str_path, "controller.baz"])
        .output()
        .unwrap();
    assert!(get.status.success());
    assert_eq!(String::from_utf8(get.stdout).unwrap().trim(), "123");

    // The file is edited by hand: no verb rewrites it and drops its comments.
    let set = Command::new(binary)
        .arg("param")
        .args(["set", str_path, "controller.mode", "\"safe\""])
        .output()
        .unwrap();
    assert!(!set.status.success(), "hypha_param has no set verb");

    let validate = Command::new(binary).arg("param").args(["validate", str_path]).output().unwrap();
    assert!(validate.status.success());
    assert_eq!(fs::read_to_string(str_path).unwrap(), initial_content);

    let _ = fs::remove_file(&tmp_path);
}

#[test]
#[allow(clippy::unwrap_used)]
fn help_exits_zero_and_usage_error_does_not() {
    let binary = env!("CARGO_BIN_EXE_hypha");

    let help = Command::new(binary).arg("param").arg("--help").output().unwrap();
    assert!(help.status.success(), "--help must exit 0");
    assert!(String::from_utf8(help.stdout).unwrap().contains("Usage:"));

    let usage = Command::new(binary).arg("param").arg("bogus").output().unwrap();
    assert!(!usage.status.success(), "an unknown verb must exit non-zero");
}

/// A directory with a root file that includes two files, removed on drop.
struct IncludeTree {
    directory: std::path::PathBuf,
}

impl IncludeTree {
    #[allow(clippy::unwrap_used)]
    fn new(name: &str, root: &str, first: &str, second: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("hypha_param_{name}_{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("root.toml"), root).unwrap();
        fs::write(directory.join("first.toml"), first).unwrap();
        fs::write(directory.join("second.toml"), second).unwrap();
        Self { directory }
    }

    fn root(&self) -> String {
        self.directory.join("root.toml").display().to_string()
    }
}

impl Drop for IncludeTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn param(arguments: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_hypha")).arg("param").args(arguments).output();
    output.unwrap_or_else(|error| unreachable_output(&error))
}

#[allow(clippy::panic)]
fn unreachable_output(error: &std::io::Error) -> std::process::Output {
    panic!("cannot run hypha param: {error}")
}

#[test]
fn validate_refuses_a_key_two_includes_set_as_a_node_does() {
    let tree = IncludeTree::new(
        "duplicate",
        "include = [\"first.toml\", \"second.toml\"]\n",
        "[controller]\ngain = 1\n",
        "[controller]\ngain = 2\n",
    );
    let output = param(&["validate", &tree.root()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.starts_with("hypha param: "), "stderr: {stderr}");
    assert!(stderr.contains("controller.gain"), "stderr: {stderr}");
}

#[test]
fn validate_get_and_dump_read_the_file_as_a_node_does() {
    let tree = IncludeTree::new(
        "merged",
        "include = [\"first.toml\", \"second.toml\"]\nname = \"${HYPHA_PARAM_TEST_NAME}\"\n",
        "[controller]\ngain = 1\n",
        "[sensor]\nrate = 10.0\n",
    );
    let root = tree.root();
    let validate = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["param", "validate", &root])
        .env("HYPHA_PARAM_TEST_NAME", "rover")
        .output();
    let validate = validate.unwrap_or_else(|error| unreachable_output(&error));
    assert!(validate.status.success(), "{}", String::from_utf8_lossy(&validate.stderr));
    assert_eq!(String::from_utf8_lossy(&validate.stdout).trim(), format!("{root}: valid"));

    let get = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["param", "get", &root, "name"])
        .env("HYPHA_PARAM_TEST_NAME", "rover")
        .output();
    let get = get.unwrap_or_else(|error| unreachable_output(&error));
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "rover");

    let dump = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["param", "dump", &root])
        .env("HYPHA_PARAM_TEST_NAME", "rover")
        .output();
    let dump = dump.unwrap_or_else(|error| unreachable_output(&error));
    let text = String::from_utf8_lossy(&dump.stdout).into_owned();
    assert!(text.contains("controller.gain=1\n"), "dump: {text}");
    assert!(text.contains("sensor.rate=10.0\n"), "dump: {text}");
    assert!(text.contains("name=rover\n"), "dump: {text}");
}
