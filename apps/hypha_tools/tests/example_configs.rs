//! The example configurations follow the rules of the framework.

#![allow(clippy::expect_used, reason = "test setup must succeed")]

use std::path::{Path, PathBuf};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// Every file under `dir` with one of `extensions`, `upstream/` and build output left out.
fn files(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    collect(dir, extensions, true, out);
}

/// As `files`, `upstream/` included: the vendored code of a port declares its endpoints.
fn files_with_upstream(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    collect(dir, extensions, false, out);
}

fn collect(dir: &Path, extensions: &[&str], skip_upstream: bool, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).expect("read an examples directory");
    for entry in entries {
        let path = entry.expect("read a directory entry").path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        let skipped =
            (skip_upstream && name == "upstream") || name == "build" || name.starts_with('.');
        if skipped {
            continue;
        }
        if path.is_dir() {
            collect(&path, extensions, skip_upstream, out);
            continue;
        }
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if extensions.contains(&extension) {
            out.push(path);
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read an example file")
}

/// The input tables of `config` without `silence_ms`, by port name. A table with
/// `topic = ""` disables its port, which then reads no limit.
fn inputs_without_limit(config: &str) -> Vec<String> {
    let table: toml::Table = config.parse().expect("an example config parses");
    let mut bare = Vec::new();
    let inputs =
        table.get("ports").and_then(|ports| ports.get("inputs")).and_then(toml::Value::as_table);
    for (name, port) in inputs.into_iter().flatten() {
        let has_limit = port.as_table().is_some_and(|port| port.contains_key("silence_ms"));
        let disabled = port.get("topic").and_then(toml::Value::as_str) == Some("");
        if !has_limit && !disabled {
            bare.push(name.clone());
        }
    }
    bare
}

/// True when a source line declares port `name` with the tag that needs no limit,
/// `unsupervised`; an `advisory` port states its limit too (#238).
fn tagged_without_limit(sources: &[String], name: &str) -> bool {
    let quoted = format!("\"{name}\"");
    sources.iter().any(|source| {
        source.lines().any(|line| {
            let declares = line.contains(&quoted);
            let tagged = line.contains("unsupervised");
            declares && tagged
        })
    })
}

/// True when a source subscribes to port `name` on a plain node (`node.subscribe<M>("name",
/// ...)`, also across lines, or an rclcpp node of the Autoware port's shim,
/// `create_subscription<M>("name", ...)`): a callback subscriber has no silence limit to
/// declare.
fn subscribed_on_a_plain_node(sources: &[String], name: &str) -> bool {
    let argument = format!("(\"{name}\"");
    sources.iter().any(|source| {
        let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
        compact.match_indices(&argument).any(|(at, _)| {
            let before = compact.get(..at).unwrap_or("");
            let call = before.rsplit([';', '{', '}']).next().unwrap_or("");
            (call.contains(".subscribe<") || call.contains("create_subscription<"))
                && call.ends_with('>')
        })
    })
}

/// The sources of the example that holds `config`: every one under its directory.
fn example_sources(config: &Path, code: &[PathBuf]) -> Vec<String> {
    let examples = examples_dir();
    let example = config
        .ancestors()
        .find(|dir| dir.parent().is_some_and(|parent| parent == examples.as_path()));
    code.iter()
        .filter(|path| example.is_some_and(|dir| path.starts_with(dir)))
        .map(|path| read(path))
        .collect()
}

/// True when no source creates a cycle: every node of the example then reads its inputs
/// through callback subscribers, which have no silence limit to declare. A port's name can
/// then come from a parameter, as the topics of the Autoware port's concatenate node do,
/// where no source spells it.
fn creates_no_cycle(sources: &[String]) -> bool {
    let cycle_api = ["CycleNode", "hypha::Cycle", "<hypha/cycle", "hypha_cycle_"];
    !sources.iter().any(|source| cycle_api.iter().any(|name| source.contains(name)))
}

/// The sources of the node that reads `config`: those whose file name starts with the
/// config's name (`control_manager.toml` -> `control_manager.cpp`, and `mrs_simulator_teleop`
/// -> `mrs_simulator.cpp`), else every source of the example's directory.
fn own_sources(config: &Path, code: &[PathBuf]) -> Vec<String> {
    let examples = examples_dir();
    let example = config
        .ancestors()
        .find(|dir| dir.parent().is_some_and(|parent| parent == examples.as_path()));
    let local: Vec<&PathBuf> =
        code.iter().filter(|path| example.is_some_and(|dir| path.starts_with(dir))).collect();
    let stem = config.file_stem().and_then(|stem| stem.to_str()).unwrap_or("");
    let named: Vec<&PathBuf> = local
        .iter()
        .copied()
        .filter(|path| {
            let source = path.file_stem().and_then(|name| name.to_str()).unwrap_or("");
            !source.is_empty() && (stem.starts_with(source) || source.starts_with(stem))
        })
        .collect();
    let chosen = if named.is_empty() { local } else { named };
    chosen.iter().map(|path| read(path)).collect()
}

#[test]
fn examples_have_no_use_sim_time_key() {
    let mut configs = Vec::new();
    files(&examples_dir(), &["toml"], &mut configs);
    assert!(!configs.is_empty(), "no example configuration found");
    for path in configs {
        let text = read(&path);
        let keyed = text.lines().any(|line| line.trim_start().starts_with("use_sim_time"));
        assert!(!keyed, "{} sets use_sim_time", path.display());
    }
}

#[test]
fn example_inputs_declare_silence_or_a_tag() {
    let mut configs = Vec::new();
    files(&examples_dir(), &["toml"], &mut configs);
    let mut code = Vec::new();
    files_with_upstream(&examples_dir(), &["cpp", "h", "hpp", "ipp"], &mut code);
    for path in configs {
        let sources = own_sources(&path, &code);
        let no_cycle = creates_no_cycle(&example_sources(&path, &code));
        for name in inputs_without_limit(&read(&path)) {
            let needs_no_limit = no_cycle
                || tagged_without_limit(&sources, &name)
                || subscribed_on_a_plain_node(&sources, &name);
            assert!(
                needs_no_limit,
                "{}: input '{name}' has no silence_ms (0 for no limit) and no unsupervised tag",
                path.display()
            );
        }
    }
}

#[test]
fn a_bare_input_is_reported() {
    let bare = inputs_without_limit("[ports.inputs.speed]\ntopic = \"/speed\"\n");
    assert_eq!(bare, ["speed"]);
    assert!(!tagged_without_limit(&[String::from("Latest<F> speed{*this, \"speed\"};")], "speed"));
    let advisory = String::from("Latest<F> speed{*this, \"speed\", advisory};");
    assert!(!tagged_without_limit(&[advisory], "speed"), "an advisory port states its limit");
    let disabled = inputs_without_limit("[ports.inputs.speed]\ntopic = \"\"\n");
    assert!(disabled.is_empty(), "a disabled port reads no limit");
    assert!(!subscribed_on_a_plain_node(
        &[String::from("Latest<F> speed{*this, \"speed\"};")],
        "speed"
    ));
}

#[test]
fn a_plain_node_subscriber_needs_no_limit() {
    let one_line = String::from("auto imu = node.subscribe<Imu>(\"imu\", onImu);");
    let wrapped = String::from(
        "auto cloud =\n    node.subscribe<Cloud>(\n        \"cloud\", onCloud\n    );",
    );
    let imu_source = [one_line];
    assert!(subscribed_on_a_plain_node(&imu_source, "imu"));
    assert!(subscribed_on_a_plain_node(&[wrapped], "cloud"));
    assert!(!subscribed_on_a_plain_node(&imu_source, "cloud"));
    let advertised = String::from("auto out = node.advertise<Imu>(\"imu\");");
    assert!(!subscribed_on_a_plain_node(&[advertised], "imu"));
}

#[test]
fn an_example_without_a_cycle_needs_no_limit() {
    let plain = [String::from("auto sub = this->create_subscription<Cloud>(topic, qos, cb);")];
    assert!(creates_no_cycle(&plain));
    let cycle = [String::from("class Fusion : public hypha::CycleNode {};")];
    assert!(!creates_no_cycle(&cycle));
    let composed = [String::from("#include <hypha/cycle.h>")];
    assert!(!creates_no_cycle(&composed));
}

#[test]
fn example_units_restart_on_failure() {
    let units = read(&examples_dir().join("mrs/launch/mrs_teleop.toml"));
    let table: toml::Table = units.parse().expect("the units file parses");
    let list = table.get("units").and_then(toml::Value::as_array).expect("[[units]]");
    for unit in list {
        let restart = unit.get("restart").and_then(toml::Value::as_str);
        assert_eq!(restart, Some("on-failure"), "unit {:?}", unit.get("name"));
    }
}

/// The `HYPHA_REPLAY_LOCKSTEP` value of each unit of `units` that takes part in a replay:
/// a node on simulated time, or the player.
fn replay_switches(units: &str) -> Vec<Option<String>> {
    let table: toml::Table = units.parse().expect("the units file parses");
    let list = table.get("units").and_then(toml::Value::as_array).expect("[[units]]");
    let mut switches = Vec::new();
    for unit in list {
        let env = unit.get("env").and_then(toml::Value::as_table);
        let variable = |name: &str| env.and_then(|env| env.get(name)).and_then(toml::Value::as_str);
        let sim_time = variable("HYPHA_USE_SIM_TIME");
        assert_ne!(sim_time, Some("follow"), "unit {:?}: follow is gone", unit.get("name"));
        let plays = unit.to_string().contains("hypha play");
        let on_sim_time = sim_time.is_some_and(|value| value != "0" && value != "false");
        if plays || on_sim_time {
            switches.push(variable("HYPHA_REPLAY_LOCKSTEP").map(str::to_owned));
        }
    }
    switches
}

#[test]
fn an_example_launch_is_all_lockstep_or_all_open_loop() {
    let mut launches = Vec::new();
    files(&examples_dir(), &["toml"], &mut launches);
    launches.retain(|path| path.parent().is_some_and(|dir| dir.ends_with("launch")));
    assert!(!launches.is_empty(), "the examples hold launch files");
    for path in launches {
        let mut switches = replay_switches(&read(&path));
        switches.dedup();
        assert!(switches.len() <= 1, "{} mixes the modes: {switches:?}", path.display());
    }
}

#[test]
fn launchers_set_no_qos_and_keep_the_default_domain() {
    let mut scripts = Vec::new();
    files(&examples_dir(), &["sh", "toml", "cpp", "h"], &mut scripts);
    for path in scripts {
        let text = read(&path);
        assert!(!text.contains("HYPHA_QOS"), "{} sets HYPHA_QOS", path.display());
    }
    let run = read(&examples_dir().join("mrs/run.sh"));
    assert!(!run.contains("HYPHA_DOMAIN"), "the MRS example stays in the default domain");
}
