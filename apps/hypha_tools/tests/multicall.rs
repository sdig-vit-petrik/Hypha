//! The one `hypha` executable: its tool list, `hy`, and a start under a link `hy<tool>`.

use std::path::PathBuf;
use std::process::Command;

const HYPHA: &str = env!("CARGO_BIN_EXE_hypha");

const TOOLS: [&str; 12] = [
    "topic",
    "top",
    "node",
    "record",
    "play",
    "convert",
    "intercept",
    "foxglove",
    "launch",
    "param",
    "clean",
    "load",
];

#[test]
#[allow(clippy::expect_used)]
fn hypha_binary_has_one_entry_per_tool() {
    let help = Command::new(HYPHA).arg("--help").output().expect("run hypha --help");
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout).into_owned();
    let listed: Vec<&str> = text
        .lines()
        .skip_while(|line| *line != "Tools:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(listed, TOOLS, "help:\n{text}");
}

#[test]
#[allow(clippy::expect_used)]
fn each_tool_answers_help_as_a_subcommand() {
    for tool in TOOLS {
        let help = Command::new(HYPHA).args([tool, "--help"]).output().expect("run the tool");
        assert!(help.status.success(), "hypha {tool} --help failed");
        let text = String::from_utf8_lossy(&help.stdout).into_owned();
        assert!(text.contains(&format!("hypha {tool}")), "hypha {tool} --help:\n{text}");
    }
}

#[test]
#[allow(clippy::expect_used)]
fn an_unknown_tool_or_no_tool_is_refused_with_the_tool_list() {
    for arguments in [vec!["bogus"], Vec::new()] {
        let output = Command::new(HYPHA).args(&arguments).output().expect("run hypha");
        assert_eq!(output.status.code(), Some(1), "hypha {arguments:?}");
        let text = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(text.contains("Usage: hypha <tool>"), "stderr: {text}");
    }
}

/// A directory of links to `hypha` under `names`, removed when dropped.
struct Links(PathBuf);

impl Links {
    #[allow(clippy::expect_used)]
    fn new(test: &str, names: &[String]) -> Self {
        let directory =
            std::env::temp_dir().join(format!("hypha_multicall_{test}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create the link directory");
        for name in names {
            std::os::unix::fs::symlink(HYPHA, directory.join(name)).expect("link to hypha");
        }
        Self(directory)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Links {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[allow(clippy::expect_used)]
fn a_start_as_hy_tool_runs_that_tool_and_names_it_hypha_tool() {
    let names: Vec<String> = TOOLS.iter().map(|tool| format!("hy{tool}")).collect();
    let links = Links::new("short", &names);
    for (tool, name) in TOOLS.iter().zip(&names) {
        let help = Command::new(links.path(name)).arg("--help").output().expect("run the link");
        assert!(help.status.success(), "{name} --help failed");
        let text = String::from_utf8_lossy(&help.stdout).into_owned();
        assert!(text.contains(&format!("hypha {tool}")), "{name} --help:\n{text}");
    }

    let missing = Command::new(links.path("hyplay")).output().expect("run hyplay");
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr).into_owned();
    assert!(stderr.contains("hypha play: --input <file> is required"), "stderr: {stderr}");
}

#[test]
#[allow(clippy::expect_used)]
fn hy_is_hypha() {
    let links = Links::new("hy", &["hy".to_owned()]);
    let help = Command::new(links.path("hy")).arg("--help").output().expect("run hy --help");
    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout).into_owned();
    assert!(text.contains("Usage: hypha <tool>"), "hy --help:\n{text}");
    let tool = Command::new(links.path("hy")).args(["param", "--help"]).output().expect("run hy");
    assert!(tool.status.success());
    assert!(String::from_utf8_lossy(&tool.stdout).contains("hypha param"));
}

#[test]
#[allow(clippy::expect_used)]
fn an_old_hypha_tool_name_runs_no_tool() {
    let links = Links::new("old", &["hypha_play".to_owned()]);
    let output = Command::new(links.path("hypha_play")).output().expect("run hypha_play");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("Usage: hypha <tool>"), "stderr: {stderr}");
}
