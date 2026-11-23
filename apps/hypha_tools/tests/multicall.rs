//! The one `hypha` executable: its tool list, and a start under an old name.

use std::path::PathBuf;
use std::process::Command;

const HYPHA: &str = env!("CARGO_BIN_EXE_hypha");

const TOOLS: [&str; 10] = [
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

#[test]
#[allow(clippy::expect_used)]
fn a_start_under_an_old_name_runs_that_tool() {
    let directory = std::env::temp_dir().join(format!("hypha_multicall_{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create the link directory");
    let linked: PathBuf = directory.join("hypha_play");
    let _ = std::fs::remove_file(&linked);
    std::os::unix::fs::symlink(HYPHA, &linked).expect("link hypha_play to hypha");

    let help = Command::new(&linked).arg("--help").output().expect("run hypha_play --help");
    let missing = Command::new(&linked).output().expect("run hypha_play");
    std::fs::remove_dir_all(&directory).expect("remove the link directory");

    assert!(help.status.success());
    let text = String::from_utf8_lossy(&help.stdout).into_owned();
    assert!(text.contains("--input"), "hypha_play --help:\n{text}");
    let stderr = String::from_utf8_lossy(&missing.stderr).into_owned();
    assert!(!missing.status.success());
    assert!(stderr.contains("hypha play: --input <file> is required"), "stderr: {stderr}");
}
