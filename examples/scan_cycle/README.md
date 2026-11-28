# Scan cycle: the reference for a cycle node and its tests

A software PLC cycle: the runtime freezes the inputs once per period and judges their
age, `step()` runs a control law on that image, and one command is sent. Copy this
example for a node that computes at one rate over its inputs. For an event-driven node
(`hypha::ApplicationNode`), copy [sensor_pipeline](../sensor_pipeline/README.md).

| File | Holds |
|---|---|
| `inc/scan_cycle/command_law.h` | the control law: pure logic, no hypha name |
| `inc/scan_cycle/scan_cycle.h` | the node: ports as members, parameters in the constructor, `step()` |
| `src/scan_cycle.cpp` | `main`: one call of `hypha::runNode` |
| `config/control.toml` | the live parameters |
| `test/test_command_law.cpp` | level 1: the control law alone |
| `test/test_scan_cycle_step.cpp` | level 2: the step through `hypha::testing::StepHarness` |
| `test/test_scan_cycle_bus.cpp` | level 3: the started cycle on the bus, on simulated time |
| `test/scan_cycle_test.toml` | the parameters of levels 2 and 3 |

## What each rule looks like here

| Rule | Here |
|---|---|
| A cycle node starts with `runNode` and nothing else | `src/scan_cycle.cpp`. `runNode` loads the file, refuses an unread key, publishes `~/state` and `~/introspection`, and returns the exit code: 0, 2 (usage), 78 (configuration refused), 75 (simulated time went back), 70 (anything else). |
| Every tuning value comes from the file and is checked | The constructor reads `gain`, `fallback_setpoint` and `command_frame` with `required<T>(key, constraint)`. A missing or refused value stops the start with 78. No default hides in the code. |
| Each input names its contract | The table below, and the comment at the top of `scan_cycle.h`. |
| The step uses its `CycleTime` | `message.header.stamp = time.stamp`. The step reads no clock, so a live run and a replay give the same stamps. |
| The step checks the verdict before it reads | `usable()` before `*port`. `*port` throws when the input is not usable; `has()` only says a value is held, also a stale one. |
| The verdict is about age, not content | A usable `NaN` speed is refused by the step itself. |

## The contract of each input

| Input | Kind and tags | Why | Never arrived | Silent longer than `silence_ms` | Recovery | `~/state` |
|---|---|---|---|---|---|---|
| `speed` | `Latest`, required | the step needs the newest measurement, not every one | command 0.0 | command 0.0; the old value stays in `lastKnown()` and is not used | the first step with a usable sample commands again; nothing latches | `WAITING`, then `DEGRADED` while not usable |
| `setpoint` | `Latest`, `optionalTopic`, `advisory` | a deployment may leave it unwired (`topic = ""`), and the node is healthy without it | the fallback setpoint | the fallback setpoint | the first step with a usable sample uses it | never degrades it |

- **`optionalTopic` and `advisory` are two decisions.** `optionalTopic` is about wiring:
  the port may have no topic. `advisory` is about health: the input does not count for
  `~/state`. A wired `optionalTopic` input still counts unless it is also `advisory`.
- **`Latest` or `Arrivals`.** Both inputs are snapshots, so both are `Latest`. An input
  whose every message must be processed (a command, an event, a sensor sample that is
  integrated) is an `Arrivals` port with a `perStep(n)` bound.
- **Stagnation is not judged here.** Neither message carries a life sign, and both
  inputs come from a hypha cycle: a sender that stopped is reported by `silence_ms`. A
  value relayed from a device or a bridge needs `setAlive()` and `stagnation_ms`
  ([execution-model.md](../../docs/execution-model.md) §6).
- **No time of its own.** The step keeps no timestamp of an input and no timeout
  parameter: `silence_ms` is the one limit. A source time that is data (interpolation, a
  computed delay) is read from the payload and kept.

## The three test levels

| Level | Test | Needs | Proves |
|---|---|---|---|
| 1 | `test_command_law.cpp` | nothing | the arithmetic, and "no speed, no motion" |
| 2 | `test_scan_cycle_step.cpp` | the parameter file | one case for each verdict of each input and for recovery: the command's value, its stamp and frame, and `inputsOk()` |
| 3 | `test_scan_cycle_bus.cpp` | shared memory | the ports reach their topics; the capture judges silence at the limit; all 50 commands of the run arrive, in order, stamped with their deadlines; health follows the input |

Level 2 is where a developer debugs: no bus, no timing, a step at a time the test
chooses. `harness.deliver(port, value, sentAt)` gives a port a value and its send time,
and each step judges it against `silence_ms` as a capture does. A port the test never
set is not judged: state `setNeverArrived(port)` when the case is about it.

Level 3 is small on purpose. It runs on simulated time (`hypha::enableSimTime`,
`hypha::setSimTime`), so a slow machine cannot change its result, and in a domain of its
own, so it never meets a system running on the host.

A node with recorded fixtures adds a golden test at level 2 with
`hypha::testing::McapReplay` ([migration guide](../../docs/ros1-migration-guide.md),
Step 6), and a rewind check at level 3 with `hypha play --clock --loop` under its
supervisor.

## Build, test, run

In the hypha tree, `./build.sh --tests` builds the node and its tests, and
`ctest --test-dir build -R scan_cycle` runs them. Against an installed SDK:

```sh
cmake -S examples/scan_cycle -B build/scan_cycle -DCMAKE_PREFIX_PATH=/path/to/installed/hypha
cmake --build build/scan_cycle && ctest --test-dir build/scan_cycle
```

Run it with the speed source of [the speed pair](../README.md), in one `HYPHA_DOMAIN`:

```sh
HYPHA_DOMAIN=demo build/examples/speed/hypha_speed_source examples/speed/config/source.toml
HYPHA_DOMAIN=demo build/examples/scan_cycle/hypha_scan_cycle examples/scan_cycle/config/control.toml
HYPHA_DOMAIN=demo target/release/hypha topic echo /scan_cycle/command
HYPHA_DOMAIN=demo target/release/hypha topic echo /scan_cycle/state
```

Stop the source: after 200 ms the command is 0.0 and `~/state` shows `DEGRADED`. Start
it again: the command and the state recover.
