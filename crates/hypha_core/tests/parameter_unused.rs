//! Keys set on purpose and not read: the file's top-level `unused` list and
//! `Parameters::ignore` keep them out of the unread keys, and nothing else does (#119).
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test setup must succeed")]

mod common;

use std::path::{Path, PathBuf};

use hypha_core::parameters::Parameters;

/// A directory under the system temp directory. It is removed when the test ends.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("hypha_unused_{}_{label}", std::process::id()));
        // A fixture left by an earlier run may be absent; that is the normal case.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture directory");
        Self { dir }
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.dir.join(relative);
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
    Parameters::from_toml_file(path).expect("load parameters")
}

#[test]
fn a_listed_key_or_table_is_not_unread_and_the_others_still_are() {
    let source = r#"
unused = ["pose.stale", "model_params"]
typo = 1

[pose]
stale = 0.01
kept = 5

[model_params]
precision = "fp32"
batch_size = 32
"#;
    let params = common::from_text(source, "unused.toml").unwrap();
    assert_eq!(params.unread_keys(), vec!["pose.kept", "typo"]);
    assert!(!params.has_list("unused"), "`unused` is not a parameter");
}

#[test]
fn a_listed_key_the_file_does_not_set_is_refused() {
    let source = "unused = [\"pose.stael\"]\n[pose]\nstale = 0.01\n";
    let refused = common::from_text(source, "stale_list.toml").unwrap_err();
    assert!(refused.path.ends_with("stale_list.toml"), "{}", refused.path);
    assert_eq!(refused.key, "unused");
    assert_eq!(refused.reason, "`pose.stael` is not set in the configuration");
}

#[test]
fn a_prefix_matches_whole_names_only() {
    let source = "unused = [\"pose\"]\n[poses]\nstale = 0.01\n";
    let refused = common::from_text(source, "prefix.toml").unwrap_err();
    assert_eq!(refused.reason, "`pose` is not set in the configuration");
}

#[test]
fn unused_must_be_an_array_of_nonempty_strings() {
    for source in ["unused = \"pose\"\npose = 1\n", "unused = [1]\n", "unused = [\"\"]\n"] {
        let refused = common::from_text(source, "shape.toml").unwrap_err();
        assert_eq!(refused.key, "unused", "{source}");
        assert_eq!(refused.reason, "the value must be an array of key names", "{source}");
    }
}

#[test]
fn an_included_file_may_list_its_own_keys_and_the_root_file_its_own() {
    let fixture = Fixture::new("include");
    fixture.write("vendor.toml", "unused = [\"vendor.stale\"]\n[vendor]\nstale = 1\nkept = 2\n");
    let root = fixture
        .write("root.toml", "include = [\"vendor.toml\"]\nunused = [\"local\"]\nlocal = 3\n");
    let params = load(&root);
    assert_eq!(params.unread_keys(), vec!["vendor.kept"]);
    let ignored = params.ignored();
    let listed: Vec<(&str, &str)> =
        ignored.iter().map(|entry| (entry.key.as_str(), entry.reason.as_str())).collect();
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert!(listed.iter().any(|(key, reason)| *key == "local" && reason.ends_with("root.toml")));
    assert!(
        listed
            .iter()
            .any(|(key, reason)| *key == "vendor.stale" && reason.ends_with("vendor.toml"))
    );
    assert!(ignored.iter().all(|entry| entry.listed_in_file));
}

#[test]
fn ignore_covers_a_key_a_table_and_a_list_entry_and_an_absent_key_is_fine() {
    let source = r#"
typo = 1

[model_params]
precision = "fp32"

[[sensors]]
topic = "/imu"
rate = 10
"#;
    let params = common::from_text(source, "ignore.toml").unwrap();
    assert!(params.ignore("model_params", "no TensorRT in this build"));
    assert!(params.ignore("sensors[0].rate", "the driver sets its own rate"));
    assert!(params.ignore("not_in_the_file", "a branch not taken"));
    assert_eq!(params.unread_keys(), vec!["sensors[0].topic", "typo"]);
    assert_eq!(params.unread_keys_under("model_params"), Vec::<String>::new());
    let ignored = params.ignored();
    assert_eq!(ignored.len(), 3);
    assert!(ignored.iter().all(|entry| !entry.listed_in_file));
}

#[test]
fn ignoring_a_key_again_keeps_the_first_reason() {
    let params = common::from_text("typo = 1\n", "again.toml").unwrap();
    assert!(params.ignore("typo", "first"));
    assert!(!params.ignore("typo", "second"));
    let reasons: Vec<String> = params.ignored().into_iter().map(|entry| entry.reason).collect();
    assert_eq!(reasons, vec!["first"]);
}
