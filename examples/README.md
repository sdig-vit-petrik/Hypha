# Hypha Examples

Consumers of the Hypha Core SDK.
The C++ examples use the installed CMake package, not Core source files.

## Which example to copy

Two examples are references: copy their structure, their entry point and their tests.
The others show one feature or measure the framework, and are not templates for an
application node.

| Example | Copy it for | What it is |
|---|---|---|
| [scan_cycle](scan_cycle/README.md) | **a cycle node** (`hypha::CycleNode`) | **Reference.** `runNode` entry point, parameters checked in the constructor, the contract of each input stated, the stamp from `CycleTime`, and tests at three levels: logic, `StepHarness`, bus. |
| [sensor_pipeline](sensor_pipeline/README.md) | **an event-driven node** (`hypha::ApplicationNode`) | **Reference.** `runNode` entry point, endpoints created in `setup()`, a worker thread started in `started()` and joined in the destructor, shared QoS policies, loss and timer counters, a start-up exit-code test, and a replay check on a recorded fixture. |
| `speed` | a second cycle node to run beside `scan_cycle` | A source and a monitor. The monitor names never-arrived and stale apart with `verdict()` and withholds its Bool rather than send a false all-clear. No tests of its own. |
| `sensor_fusion` | composed `hypha::Cycle`s built from a `[[sensors]]` list | A feature demonstration with a generator and a probe. Its mains return 0 or 1 for its script; an application returns the codes of `hypha/application_node.h`. |
| [mrs](mrs/README.md) | nothing | A port of upstream ROS code behind a shim, kept close to the original. Its inputs are `unsupervised` with the upstream's own checks, which is why it reads `has()`; new code declares `silence_ms` and reads `usable()`. |
| [autoware](autoware/README.md) | nothing | A port of upstream Autoware (ROS 2) nodes on an rclcpp shim over a plain `hypha::Node`. The upstream sources are vendored unchanged; the port's edits are patch files. |
| `sim_twin`, `bench`, `large_payload` | nothing | Measurements of the framework, driven by scripts that read 0 or 1. |
| `load` | a workload file of `hypha load` | `smoke.toml`: three processes, a slow reader and the recorder; `hypha load examples/load/smoke.toml --report smoke.json` ([tools](../docs/tools.md)). |

What a reference shows, and where to find the rule:

1. **Entry point.** A cycle node and an event-driven node are each one call of
   `hypha::runNode`: the exit codes, `~/introspection`, `~/state` and the refusal of an
   unread key come with it ([migration guide](../docs/ros1-migration-guide.md) §10.2).
2. **Input contracts.** `Latest` for a snapshot, bounded `Arrivals` for work on each
   message, `optionalTopic` for wiring, `advisory` for health; and what never-arrived,
   silence, stagnation and recovery do to each output.
3. **Time.** `CycleTime` for the step's decisions and the stamps it writes; a source
   stamp stays data; `steadyNow()` for real elapsed time.
4. **Tests at three levels.** The logic alone; the step through `StepHarness` (and
   `McapReplay` for recorded fixtures); a small bus test. They assert output content and
   stamps, health, and loss.

## The speed pair and the scan cycle

```sh
cmake -S examples -B build/examples -DCMAKE_PREFIX_PATH=/path/to/installed/hypha
cmake --build build/examples
```

The speed pair and `scan_cycle` are `hypha::CycleNode` classes started with
`hypha::runNode`. Their inputs (`Latest`) and outputs (`Output`) are members.
The runtime freezes every input before it calls `step()`. An output publishes
only when `step()` calls `send()` on it.
`scan_cycle` commands zero for missing/stale speed and uses its fallback setpoint
when the optional setpoint is switched off or not usable. The monitor suppresses its Bool status when
speed is unavailable: false must not mean an all-clear without a measurement.
Set `debug_enabled = true` to emit the monitor's `~/debug` speed output;
the `topic` key of `[ports.outputs.debug]` can select another topic.
Warning logs show the monitor's application-defined bitmask in hexadecimal:
`0x1` marks an over-limit speed, `0x2` stale input and `0x4` input never seen.

Run from the repository root, in separate terminals:

```sh
HYPHA_DOMAIN=demo build/examples/speed/hypha_speed_source examples/speed/config/source.toml
HYPHA_DOMAIN=demo build/examples/speed/hypha_speed_monitor examples/speed/config/monitor.toml
HYPHA_DOMAIN=demo build/examples/scan_cycle/hypha_scan_cycle examples/scan_cycle/config/control.toml
```

Each takes one TOML file. An input port reads its topic from `[ports.inputs.<name>]`
and an output port from `[ports.outputs.<name>]`. Start fails when a `topic` is
missing. An empty `topic` (`topic = ""`) switches off a port declared with
`optionalTopic`, on purpose; `scan_cycle` then uses its fallback setpoint. `[node].name` selects the
instance name, and `~/command`, `~/status` and `~/debug` resolve beneath that
node name. All communicating
processes, including observation tools, must use the same `HYPHA_DOMAIN`.
Use a different domain for an independent system on the same host. Give
simultaneous instances distinct node names and corresponding topic mappings.

The supplied `[log]` tables enable console logging and rotating files relative
to each TOML file's directory. Module names use `HYPHA_LOG_MODULE_INIT`; the shared
runtime owns levels, sinks and rotation. SIGINT/SIGTERM stop the executor and
release the node through RAII. Stopping the source leaves explicit stale input
verdicts in the control nodes after their configured age limits.

Each directory here can also be configured on its own.

`sensor_pipeline` is an event-driven `hypha::ApplicationNode`: a 400 Hz IMU, 10 Hz
point clouds, one timer and one worker thread that looks transforms up and publishes. It
prints its loss, timer and hand-over counters, and its `replay_check.sh` replays a
recorded fixture on simulated time and compares two runs by header stamp. Its
[README](sensor_pipeline/README.md) has the commands.

## Build and CI

The examples are not shipped in an image. `-DHYPHA_BUILD_EXAMPLES=ON` builds
them with the SDK in the repository's CMake tree; the `test:cpp` job does
that. `./examples/check.sh` builds every C++ example against
an installed SDK, which is what an external consumer sees. Pass the prefix
unless it is on the default CMake search path:
`./examples/check.sh --prefix /path/to/installed/hypha`.
