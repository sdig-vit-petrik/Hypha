//! `hypha clean`'s command line: what it prints and its exit codes, on scratch domains
//! of this test, `t_<pid of this test>_k..._`. The keep rules themselves are tested in
//! `crates/hypha_node/tests/clean_domains.rs`.

#![allow(clippy::expect_used, reason = "a failed setup is a test failure")]

use std::path::PathBuf;
use std::process::{Command, Output};

const HYPHA: &str = env!("CARGO_BIN_EXE_hypha");

fn event_file(domain: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/iceoryx2/iox2-{domain}-x.event"))
}

fn touch(domain: &str) -> PathBuf {
    let path = event_file(domain);
    std::fs::create_dir_all("/tmp/iceoryx2").expect("create the iceoryx2 root");
    std::fs::write(&path, b"").expect("create the file");
    path
}

fn run(arguments: &[&str], domain: Option<&str>) -> (Option<i32>, String, String) {
    let mut command = Command::new(HYPHA);
    command.arg("clean").args(arguments);
    if let Some(domain) = domain {
        command.env("HYPHA_DOMAIN", domain);
    }
    let Output { status, stdout, stderr } = command.output().expect("run hypha clean");
    let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
    (status.code(), text(stdout), text(stderr))
}

#[test]
fn it_says_what_it_removed_and_exits_0_also_when_nothing_is_left() {
    let prefix = format!("t_{}_kr_", std::process::id());
    let dead = format!("{prefix}1_x");
    let file = touch(&dead);

    let (code, stdout, stderr) = run(&["--domain", &prefix], None);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout.contains(&format!("hypha clean: removed domain {dead} (1 resource)")),
        "{stdout}"
    );
    assert!(!file.exists());

    let (code, stdout, _) = run(&["--domain", &prefix], None);
    assert_eq!(code, Some(0), "stdout: {stdout}");
    assert!(
        stdout.contains(&format!("nothing to remove of the domains under '{prefix}'")),
        "{stdout}"
    );
}

#[test]
fn it_says_why_it_kept_a_domain_and_exits_2() {
    let pid = std::process::id();
    let prefix = format!("t_{pid}_kk_");
    let running = format!("{prefix}{pid}_x");
    let file = touch(&running);

    let (code, stdout, stderr) = run(&["--domain", &prefix], None);
    let _ = std::fs::remove_file(&file);
    assert_eq!(code, Some(2), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout
            .contains(&format!("hypha clean: kept domain {running}: its run, pid {pid}, is alive")),
        "{stdout}"
    );
}

#[test]
fn without_domain_it_cleans_the_domain_of_hypha_domain_alone() {
    let pid = std::process::id();
    let mine = format!("t_{pid}_ke");
    let neighbour = format!("t_{pid}_ke_more");
    let file = touch(&mine);
    let neighbour_file = touch(&neighbour);

    let (code, stdout, stderr) = run(&[], Some(&mine));
    let neighbour_stays = neighbour_file.exists();
    let _ = std::fs::remove_file(&neighbour_file);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains(&format!("removed domain {mine}")), "{stdout}");
    assert!(!file.exists());
    assert!(neighbour_stays, "a domain that only starts with HYPHA_DOMAIN is gone");
}

#[test]
fn an_invalid_prefix_or_domain_is_refused_with_exit_1() {
    let (code, _, stderr) = run(&["--domain", "Not-A-Prefix"], None);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("'Not-A-Prefix' is not a domain prefix"), "{stderr}");

    let (code, _, stderr) = run(&[], Some("Bad-Domain"));
    assert_eq!(code, Some(1));
    assert!(stderr.contains("hypha clean: "), "{stderr}");
}
