//! A starting node reclaims its own dead runs and every dead CLI tool node, and nothing else.

use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;
use iceoryx2::config::Config;
use iceoryx2::node::NodeView as _;
use iceoryx2::prelude::{CallbackProgression, FilePath, SemanticString as _};

/// Installs a host config that turns on every iceoryx2 dead-node cleanup.
/// `Config::global_config()` is set once per process, so this must run first.
#[allow(clippy::expect_used)]
fn install_host_config_with_dead_node_cleanup() {
    let config_dir = std::env::temp_dir().join(unique_name("stale_reclaim_config"));
    let config_path = config_dir.join("iceoryx2.toml");
    std::fs::create_dir_all(&config_dir).expect("create temp config directory");
    std::fs::write(
        &config_path,
        "[global.node]\n\
         cleanup-dead-nodes-on-creation = true\n\
         cleanup-dead-nodes-on-destruction = true\n\
         [global.service]\n\
         cleanup-dead-nodes-on-open = true\n",
    )
    .expect("write temp iceoryx2 config file");
    let config_path_str = config_path.to_str().expect("temp config path is valid UTF-8");
    let file_path =
        FilePath::new(config_path_str.as_bytes()).expect("temp config path is a valid FilePath");
    Config::setup_global_config_from_file(&file_path)
        .expect("install the test's global iceoryx2 config before anything else uses it");
    // Read only at the call above, so it can be removed right away.
    let _ = std::fs::remove_dir_all(&config_dir);
}

/// Registers a node called `name` in a child process and SIGKILLs the child,
/// so the node is left behind dead.
#[allow(clippy::expect_used)]
fn crash_node(name: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha_iceoryx2_crash_node_helper"))
        .arg(name)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn crash_node_helper");

    // Wait for the helper's "ready", so the kill is race-free.
    let stdout = child.stdout.take().expect("helper stdout piped");
    let mut reader = BufReader::new(stdout);
    let mut ready_line = String::new();
    reader.read_line(&mut ready_line).expect("read ready line from helper");
    assert_eq!(ready_line.trim(), "ready");

    // SIGKILL: no destructors run, so the node's resources are left behind.
    child.kill().expect("kill crash_node_helper");
    child.wait().expect("reap killed helper");

    // Let the OS release the killed PID before iceoryx2's liveliness check.
    std::thread::sleep(Duration::from_millis(200));
}

/// True when the registry lists a dead node called `name`.
#[allow(clippy::expect_used)]
fn is_listed_dead(config: &Config, name: &str) -> bool {
    let mut found_dead = false;
    iceoryx2::node::Node::<iceoryx2::service::ipc_threadsafe::Service>::list(config, |view| {
        if let iceoryx2::node::NodeState::Dead(dead_view) = &view {
            let has_the_name =
                dead_view.details().as_ref().is_some_and(|details| details.name().as_str() == name);
            if has_the_name {
                found_dead = true;
            }
        }
        CallbackProgression::Continue
    })
    .expect("list the node registry");
    found_dead
}

#[test]
#[allow(clippy::expect_used)]
fn a_starting_node_reclaims_its_own_dead_runs_and_every_dead_cli_tool_node() {
    install_host_config_with_dead_node_cleanup();
    let config = hypha_iceoryx2::domain::config().expect("the domain's iceoryx2 configuration");
    assert!(
        !config.global.node.cleanup_dead_nodes_on_creation,
        "the domain turns cleanup on creation off"
    );
    assert!(
        !config.global.node.cleanup_dead_nodes_on_destruction,
        "the domain turns cleanup on drop off"
    );
    assert!(
        !config.global.service.cleanup_dead_nodes_on_open,
        "the domain turns cleanup on open off"
    );

    let application = unique_name("stale_reclaim_app");
    let cli_tool = format!("_hyphacli_{}", unique_name("stale_reclaim_tool"));
    crash_node(&application);
    crash_node(&cli_tool);
    assert!(is_listed_dead(config, &application), "the killed application node must be dead");
    assert!(is_listed_dead(config, &cli_tool), "the killed CLI tool node must be dead");

    // A node of another name reclaims the dead CLI tool node, and only that.
    let _observer =
        Iox2Transport::with_name(&unique_name("stale_reclaim_observer")).expect("observer node");
    assert!(!is_listed_dead(config, &cli_tool), "any node start reclaims a dead CLI tool node");
    assert!(
        is_listed_dead(config, &application),
        "a node of another name must leave the dead application node listed"
    );

    // The next run of the application reclaims its dead one.
    let _restart = Iox2Transport::with_name(&application).expect("restarted application node");
    assert!(!is_listed_dead(config, &application), "a node reclaims the dead runs of its name");
}
