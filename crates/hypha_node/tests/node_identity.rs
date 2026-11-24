//! A node's name, `node.name`, duplicate-name detection and the `~/` topics a node owns.
//! The duplicate cases run a child: this test binary, started with a role.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hypha_node::safe;
use hypha_node::sdk::BusNode;
use hypha_testkit::unique_name;

const ROLE: &str = "HYPHA_NODE_IDENTITY_ROLE";
const NAME: &str = "HYPHA_NODE_IDENTITY_NAME";
/// The line a holding child prints once its node exists.
const HOLDING: &str = "node_identity_holding";

/// The name of a node created as `code_name` with the parameter file `parameters`.
fn node_with_parameters(code_name: &str, parameters: &str) -> Result<String, String> {
    let path = std::env::temp_dir().join(format!("{}.toml", unique_name("node_identity")));
    std::fs::write(&path, parameters).map_err(|cause| cause.to_string())?;
    let created = match path.to_str() {
        Some(text) => safe::node_create(code_name, Some(text), None).map_err(|e| format!("{e:?}")),
        None => Err("the parameter path is not UTF-8".to_owned()),
    };
    let _removed = std::fs::remove_file(path);
    let node = created?;
    let name = safe::node_name(&node).to_owned();
    safe::node_shutdown(&node);
    safe::node_destroy(&node);
    Ok(name)
}

/// A bus domain of this test process alone, so no other test or system shares it.
fn test_domain(label: &str) -> String {
    format!("t_{}_{label}", std::process::id())
}

#[allow(clippy::expect_used)]
fn spawn_child(role: &str, name: &str, domain: Option<&str>) -> std::process::Child {
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["node_identity_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(NAME, name);
    if role == "hold" {
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
    }
    match domain {
        Some(domain) => command.env("HYPHA_DOMAIN", domain),
        None => command.env_remove("HYPHA_DOMAIN"),
    };
    command.spawn().expect("spawn child")
}

/// The body of every child process. Without a role it does nothing.
#[test]
#[allow(clippy::expect_used)]
fn node_identity_child() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let name = std::env::var(NAME).expect("name");
    match role.as_str() {
        "hold" => {
            // Hold the name until the parent closes stdin, so a slow machine
            // cannot let the hold expire before the other children check it.
            let _node = BusNode::new(&name).expect("holding node");
            let mut stdout = std::io::stdout();
            writeln!(stdout, "\n{HOLDING}").expect("report holding");
            stdout.flush().expect("flush holding");
            let mut rest = Vec::new();
            let _read = std::io::stdin().read_to_end(&mut rest);
        }
        "expect_refused" => assert!(BusNode::new(&name).is_err(), "a duplicate name was accepted"),
        "expect_accepted" => assert!(BusNode::new(&name).is_ok(), "a free name was refused"),
        _ => assert!(role.is_empty(), "unknown role {role}"),
    }
}

/// A child that holds `name` in `domain` until [`release`] ends it.
struct Holder {
    child: std::process::Child,
    stdout: BufReader<std::process::ChildStdout>,
}

#[allow(clippy::expect_used)]
fn hold(name: &str, domain: &str) -> Holder {
    let mut child = spawn_child("hold", name, Some(domain));
    // Wait until the holder's node exists; a fixed delay raced slow CI runs.
    let mut stdout = BufReader::new(child.stdout.take().expect("holder stdout"));
    let mut holding = false;
    let mut line = String::new();
    while !holding {
        line.clear();
        let read = stdout.read_line(&mut line).expect("holder output");
        if read == 0 {
            break;
        }
        holding = line.trim_end() == HOLDING;
    }
    assert!(holding, "the holder exited before its node existed");
    Holder { child, stdout }
}

#[allow(clippy::expect_used)]
fn release(mut holder: Holder) {
    drop(holder.child.stdin.take());
    // Drain the holder's remaining output so its harness never writes to a closed pipe.
    let mut rest = Vec::new();
    holder.stdout.read_to_end(&mut rest).expect("holder output");
    assert!(holder.child.wait().expect("holder exit").success());
}

#[test]
#[allow(clippy::expect_used)]
fn a_name_an_alive_node_of_another_process_holds_is_refused() {
    let name = unique_name("duplicate");
    let (left, right) = (test_domain("left"), test_domain("right"));
    let holder = hold(&name, &left);
    let same_domain = spawn_child("expect_refused", &name, Some(&left)).wait().expect("exit");
    let other_domain = spawn_child("expect_accepted", &name, Some(&right)).wait().expect("exit");
    release(holder);
    assert!(same_domain.success(), "a duplicate name in one domain was accepted");
    assert!(other_domain.success(), "the same name in another domain was refused");
}

#[test]
#[allow(clippy::expect_used)]
fn any_number_of_processes_hold_one_cli_tool_name() {
    let name = format!("_hyphacli_{}", unique_name("tool"));
    let left = test_domain("left");
    let holder = hold(&name, &left);
    let second = spawn_child("expect_accepted", &name, Some(&left)).wait().expect("exit");
    release(holder);
    assert!(second.success(), "a second process of a CLI tool was refused");
}

#[test]
fn an_invalid_node_name_is_refused() {
    let long_tool = format!("_hyphacli_{}", "a".repeat(55));
    for invalid in [
        "",
        "Speed",
        "1speed",
        "speed-controller",
        "/speed",
        &"a".repeat(65),
        "_hyphacli_",
        "_hyphacli_Top",
        "_other_tool",
        &long_tool,
    ] {
        assert!(BusNode::new(invalid).is_err(), "'{invalid}' was accepted");
    }
}

#[test]
#[allow(clippy::expect_used)]
fn the_node_name_parameter_overrides_the_code_name() {
    let override_name = unique_name("ctrl_front");
    let name =
        node_with_parameters("speed_controller", &format!("[node]\nname = \"{override_name}\"\n"))
            .expect("node");
    assert_eq!(name, override_name);
}

#[test]
#[allow(clippy::expect_used)]
fn the_code_name_stays_without_the_parameter() {
    let name = unique_name("plain");
    let node = BusNode::new(&name).expect("node");
    assert_eq!(node.name(), Ok(name));
}

#[test]
fn a_node_name_parameter_that_is_not_text_or_not_valid_is_refused() {
    assert!(node_with_parameters("speed_controller", "[node]\nname = 5\n").is_err());
    assert!(node_with_parameters("speed_controller", "[node]\nname = \"Bad-Name\"\n").is_err());
}

#[test]
#[allow(clippy::expect_used)]
fn a_node_scoped_topic_resolves_under_the_node_name() {
    let name = unique_name("scoped");
    let node = BusNode::new(&name).expect("node");
    let mut publisher = node
        .publish_raw("~/speed", "std_msgs/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = node.subscribe_raw(&format!("/{name}/speed"), 2).expect("subscriber");
    assert!(publisher.publish(b"x"));
    let started = Instant::now();
    let mut received = false;
    while !received && started.elapsed() < Duration::from_secs(2) {
        received = matches!(subscriber.try_receive(), Ok(Some(_)));
    }
    assert!(received, "nothing arrived on /<node>/speed");
}

#[test]
#[allow(clippy::expect_used)]
fn a_tilde_without_a_slash_is_refused() {
    let node = BusNode::new(&unique_name("tilde")).expect("node");
    assert!(node.subscribe_raw("~", 1).is_err());
    assert!(node.subscribe_raw("~speed", 1).is_err());
}

#[test]
#[allow(clippy::expect_used)]
fn a_cli_tool_node_has_no_node_scoped_topics() {
    let name = format!("_hyphacli_{}", unique_name("scoped"));
    let node = BusNode::new(&name).expect("CLI tool node");
    assert_eq!(node.name(), Ok(name));
    assert!(
        node.publish_raw("~/speed", "std_msgs/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
            .is_err()
    );
    assert!(node.subscribe_raw("~/speed", 1).is_err());
    // A public name still works.
    let topic = format!("/{}", unique_name("cli_public"));
    assert!(
        node.publish_raw(&topic, "std_msgs/Bytes", "d9d213e8c146065ec958333c617eb1a6", "").is_ok()
    );
}
