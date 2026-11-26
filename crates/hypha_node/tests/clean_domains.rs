//! `hypha clean`'s rules: the resources of a domain nobody uses go, and a domain a live
//! node holds, a domain whose run lives, a domain outside the prefix and the domain
//! "none" stay.
//!
//! It works on scratch domains of its own, `t_<pid of this test>_c..._`, so a clean of
//! the test root `t_` by another run keeps them while this test runs, and removes what
//! it created. The nodes are real: the peer `tests/support/kill_peer.rs` creates a node
//! and a publisher in a domain, and a SIGKILL leaves that domain to a dead node. It
//! needs writable iceoryx2 shared-memory paths (`/dev/shm`, `/tmp/iceoryx2`), outside a
//! sandbox.

#![allow(clippy::expect_used, reason = "a failed setup is a test failure")]

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use hypha_node::sdk::clean::{DomainSelection, KeepReason, clean};

const ROOT: &str = "/tmp/iceoryx2";

/// A peer with a node and a publisher in `domain`, started when it said `ready`.
fn start_peer(domain: &str, node: &str) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha_node_kill_peer"))
        .args(["flood", node, "/clean_probe", "c"])
        .env("HYPHA_DOMAIN", domain)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the peer");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("the peer's stdout is piped"))
        .read_line(&mut line)
        .expect("read the peer's first line");
    assert_eq!(line.trim(), "ready", "the peer in {domain} did not start");
    child
}

/// SIGKILL: no destructor runs, so the node and its resources stay behind, dead.
fn kill(mut child: Child) {
    child.kill().expect("kill the peer");
    child.wait().expect("reap the peer");
}

fn touch(path: &Path) {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).expect("create the directory");
    }
    std::fs::write(path, b"").expect("create the file");
}

/// The files of this test that the clean must not remove, removed when the test ends.
struct Scratch {
    prefix: String,
    paths: Vec<PathBuf>,
    holder: Option<Child>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(holder) = self.holder.take() {
            kill(holder);
        }
        let _ = clean(&DomainSelection::Prefix(self.prefix.clone()));
        for path in &self.paths {
            let _ = std::fs::remove_dir_all(path).or_else(|_| std::fs::remove_file(path));
        }
    }
}

#[test]
fn a_domain_nobody_uses_goes_and_one_in_use_or_outside_the_prefix_stays() {
    let pid = std::process::id();
    let prefix = format!("t_{pid}_cl_");
    let dead = format!("{prefix}1_dead");
    let held = format!("{prefix}1_held");
    let running = format!("{prefix}{pid}_run");
    let other = format!("t_{pid}_cx_other");
    let root = Path::new(ROOT);
    let shared_node_dir = root.join(format!("nodes/cleantest{pid}"));
    let only_node_dir = root.join(format!("nodes/cleantest{pid}_only"));
    let default_file = root.join(format!("iox2_cleantest{pid}.event"));
    let mut scratch = Scratch {
        prefix: prefix.clone(),
        paths: vec![
            shared_node_dir.clone(),
            only_node_dir.clone(),
            default_file.clone(),
            root.join(format!("iox2-{running}-x.event")),
            root.join(format!("iox2-{other}-x.event")),
        ],
        holder: None,
    };

    kill(start_peer(&dead, "clean_dead_node"));
    scratch.holder = Some(start_peer(&held, "clean_held_node"));
    let holder_pid = scratch.holder.as_ref().map(std::process::Child::id);
    let schema_file = root.join(format!("iox2-{dead}-_hypha_schemas/schema"));
    let dead_node_file = shared_node_dir.join(format!("iox2-{dead}-1.port_tag"));
    let foreign_node_file = shared_node_dir.join(format!("iox2_cleantest{pid}.port_tag"));
    let only_node_file = only_node_dir.join(format!("iox2-{dead}-2.port_tag"));
    for path in [&schema_file, &dead_node_file, &foreign_node_file, &only_node_file, &default_file]
    {
        touch(path);
    }
    for domain in [&running, &other] {
        touch(&root.join(format!("iox2-{domain}-x.event")));
    }

    let report = clean(&DomainSelection::Prefix(prefix.clone()));

    assert!(report.failures.is_empty(), "failures: {:?}", report.failures);
    let removed: Vec<&str> = report.removed.iter().map(|domain| domain.domain.as_str()).collect();
    assert_eq!(removed, [dead.as_str()], "report: {report:?}");
    let kept: Vec<(&str, &KeepReason)> =
        report.kept.iter().map(|kept| (kept.domain.as_str(), &kept.reason)).collect();
    let alive = KeepReason::NodeAlive {
        name: "clean_held_node".to_owned(),
        pid: holder_pid.and_then(|pid| i32::try_from(pid).ok()),
    };
    let run = KeepReason::RunAlive(pid);
    // `CleanReport::kept` lists the domains by name, so the expected list is sorted by name
    // the same way: the order then does not depend on the pid in the domain names.
    let mut expected = [(held.as_str(), &alive), (running.as_str(), &run)];
    expected.sort_by_key(|(domain, _)| *domain);
    assert_eq!(kept, expected);

    assert!(!schema_file.exists(), "a dead domain's directory stays");
    assert!(!dead_node_file.exists(), "a dead domain's node file stays");
    assert!(!only_node_dir.exists(), "a node directory it emptied stays");
    assert!(foreign_node_file.exists(), "another domain's node file is gone");
    assert!(
        root.join(format!("iox2-{running}-x.event")).exists(),
        "the domain of a live run is gone"
    );
    assert!(
        root.join(format!("iox2-{other}-x.event")).exists(),
        "a domain outside the prefix is gone"
    );
    assert!(default_file.exists(), "a file of the domain \"none\" is gone");

    let again = clean(&DomainSelection::Prefix(dead.clone()));
    assert!(again.removed.is_empty() && again.kept.is_empty(), "the dead domain left {again:?}");

    kill(scratch.holder.take().expect("the holder runs"));
    let after_holder = clean(&DomainSelection::Exact(held.clone()));
    let removed: Vec<&str> =
        after_holder.removed.iter().map(|domain| domain.domain.as_str()).collect();
    assert_eq!(removed, [held.as_str()], "the held domain once its node died: {after_holder:?}");
}
