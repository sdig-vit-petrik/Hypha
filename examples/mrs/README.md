# MRS UAV System on hypha

One simulated UAV takes off and flies a goto on hypha. The trackers, controllers and
state estimator are upstream MRS code (the `ros1` branches of
[ctu-mrs](https://github.com/ctu-mrs), heads of 2026-10-22); the managers that host
them are native hypha `CycleNode`s; the simulator is a native model. A mission process
checks the flight and exits 0 or 1.

## What runs

```text
            hw_api/ground_truth (trigger)            uav_state (trigger)
 mrs_simulator ───────────────────► estimation_manager ───────────► control_manager
 (clock driver,                     ground_truth estimator          NullTracker, LandoffTracker,
  100 Hz, hw_api/*)                                                 LineTracker, Se3Controller,
      ▲                                                             EmergencyController (Se3),
      │ hw_api/attitude_rate_cmd (read at the next step)            FailsafeController
      └─────────────────────────────────────────────────────────────────┘
                                  ▲ services over topics ▲
                     uav_manager (100 Hz, takeoff)    mission (10 Hz, the scenario)

 estimation_manager ── odom_main ──► transform_manager ── /tf, /tf_static ──► control_manager
                                     (triggered)                              (Transformer,
                                                                               safety area)
```

| Process | Cycle | Source |
|---|---|---|
| `mrs_simulator` | rate, 100 Hz, `/clock` driver | `simulator/`: rigid body, four first-order motors, ground plane, RK4 at 1 ms, a rate controller and mixer; also the hardware API (`hw_api/*`, arming, offboard) |
| `uav1_estimation_manager` | triggered on `hw_api/ground_truth`, fallback after 100 ms | `managers/estimation_manager.cpp` around upstream `GroundTruth` |
| `uav1_control_manager` | triggered on `uav_state`, fallback (failsafe) after 100 ms | `managers/control_manager.cpp` around upstream trackers and controllers |
| `uav1_transform_manager` | triggered on `estimation_manager/odom_main`, fallback after 100 ms | `managers/transform_manager.cpp`: upstream `TransformManager`'s frames for this flight |
| `uav1_uav_manager` | rate, 100 Hz | `managers/uav_manager.cpp`: upstream takeoff as a state machine |
| `uav1_mission` | rate, 10 Hz | `managers/mission.cpp`: upstream `TestGeneric::takeoff()` and `gotoAbs()` |
| `uav1_teleop` | rate, 20 Hz | `managers/teleop.cpp`: keyboard flight in place of the mission (`teleop` scenario) |

Every process except the simulator runs on simulated time. The only delay in the
control loop is the simulator's step: a command stamped T is applied at T + 10 ms.
Unpaced, the flight (about 15 s simulated) takes about 2 s.

## How it is built

- `shim/`: the part of roscpp the upstream code touches. `ros::Time::now()` is the
  scheduled time of the running step. `ROS_*` log through hypha. `PLUGINLIB_EXPORT_CLASS`
  registers into `mrs::PluginRegistry`.
- `host/`: `mrs::PluginHost` turns a plugin's subscribers, publishers, timers and
  services into ports and dividers of the hosting manager's cycle; creating one after
  the cycle started throws. `mrs::Serve` / `mrs::Request` carry services as
  request/result topics (`mrs_bridge/Call`), with the caller's identity, a sequence id
  and a framework-time timeout. `mrs_lib` wrappers (`SubscribeHandler`,
  `PublisherHandler`, `ParamLoader`, `Transformer`) sit on top. `Transformer` reads the
  host cycle's `hypha::Transforms` port (`/tf`, `/tf_static`), so it looks up in a step
  only, never waits, and keeps upstream's rules: default frame, default prefix,
  identity for equal frames, and with `retryLookupNewest` a failed lookup at the stamp
  retried at the newest common time. `latlon_origin` is not ported.
- `config/`: one TOML per process. Each holds exactly the keys the ported code loads,
  converted once from the upstream YAML layers (first file with a key wins); a missing
  key fails start-up with its name, an unread one too. A plugin output needs a
  `[ports.outputs.<topic with dots>]` table: a hypha `Output` takes its topic from
  configuration, so every topic a process publishes is visible in its TOML.
- `upstream/<repository>/`: vendored sources with their `LICENSE` and the commit they
  come from.
  `mrs_uav_gazebo_simulator/` holds only the x500 model, converted to a URDF.

## How to run

Against an installed SDK, with Eigen 3:

```bash
./build.sh && cmake --install build --prefix /tmp/hypha-sdk
cmake -S examples -B build/examples -DCMAKE_PREFIX_PATH=/tmp/hypha-sdk && cmake --build build/examples --parallel
examples/mrs/run.sh build/examples/mrs --checks
```

`run.sh <bin-dir> [scenario] [--checks]` starts the simulator with the participants
the scenario needs, waits for the mission and returns its exit code. Scenarios:
`estimation` (on the ground, `uav_state` flows), `takeoff` (through control_manager
alone), `takeoff_via_uav_manager` (and a second takeoff refused while taking off),
`takeoff_goto` (the default: takeoff, then goto `[3.0, 2.0, 2.5, 0.0]` and hold it
within 0.1 m for 1 s), `goto_outside_safety_area` (takeoff, then `control_manager/goto`
to `[100, 0, 2, 0]` must answer `success = false`). Logs go to `<bin-dir>/logs/`.
`examples/check.sh` runs the checks and every scenario above; `./test.sh cpp` runs it
against the installed SDK, so the MRS flights are part of hypha's regular suite.
A flight runs in the caller's `HYPHA_DOMAIN`; `examples/check.sh` flies each scenario in
a test domain of its own, `t_<pid of the gate>_<n>`, so it never meets a system running
on the host.

## Fly it from the keyboard

```bash
PATH=/tmp/hypha-sdk/bin:$PATH examples/mrs/launch.sh up --attach
examples/mrs/launch.sh down     # stop
```

`launch.sh [up [--attach] | attach | down]` runs `hypha launch` (tmux 3.0 or newer) on
the static units file [`launch/mrs_teleop.toml`](launch/mrs_teleop.toml): the
simulator, the four managers, `hypha foxglove` and `mrs_teleop`, one tmux window
each with its live log. Every unit has `restart = "on-failure"`: a unit that exits 70
is started again with backoff, one that exits 78 (a refused configuration) is not; so
after you quit teleop (exit 0, not restarted), the simulator ends with `Timeout` and is
started again until `launch.sh down`.
It puts the build's executables on `PATH`
(`MRS_BUILD_DIR`, default `build/examples/mrs`). The simulator reads
`config/mrs_simulator_teleop.toml`: `/clock` paced at real time
(`clock.real_time_factor = 1.0`) with these processes as participants; every process
still runs on simulated time in lockstep with it. `Ctrl-b w` lists the windows, and
`Ctrl-b d` detaches and leaves the flight running.

`mrs_teleop` takes the mission's place and shows a status line: phase, position,
heading, goal, active tracker and the last refusal. Its log goes to
`<build>/logs/teleop.log`.

| Key | Action |
|---|---|
| `t` | takeoff: arming, `control_manager/toggle_output`, offboard, `uav_manager/takeoff` |
| `w` `s` / up down | forward / back by `step_m` (0.5 m) in the heading frame |
| `a` `d` / left right | left / right |
| `r` `f` | up / down |
| `q` `e` | heading left / right by `yaw_step_deg` (15°) |
| space | hold the current position |
| Ctrl-C | quit teleop; the simulator then ends with `Timeout`, and `launch.sh down` stops the rest |

Each key moves the goal and sends `control_manager/goto`; keys pressed while a goto is
pending go out together in the next one. A goal outside the safety area is refused and
the goal returns to the last accepted one. There is no landing: the port has no land
service.

`hypha foxglove` runs in its own window: open Foxglove Studio and connect to
`ws://localhost:8765`, then import `foxglove_layout.json` (Layouts, Import from file). Its 3D panel follows `uav1/fcu` with the UAV from `/robot_description` and a ground
grid in `uav1/world_origin`, 0.15 m below `fcu` where the legs end (the simulator's ground
contact is the `fcu` origin); frame labels and the origin frames that coincide on the
ground are hidden. A plot of the `uav_state` position sits beside it. The layout sets the
mesh up axis to Z-up (Gazebo and ROS meshes; the viewer's default is Y-up); the viewer
applies it after a restart, so restart it once after the import.

The model is the Holybro X500 v2 of MRS's Gazebo simulator
(`upstream/mrs_uav_gazebo_simulator/x500.urdf`, see its README). `mrs_simulator` publishes
it latched once (config key `robot_description`, absent: no model). Its meshes are local
`file://` paths, so the viewer must run on the same machine (Foxglove desktop).

## Gains

No tuning was needed: Se3Controller flies with the upstream gains of
`mrs_uav_controllers` `config/{private,public}/se3_controller.yaml`; the simulator's rate
controller uses `kp = [20, 20, 10]`, `ki = [2, 2, 1]`, `kd = 0` (`config/mrs_simulator.toml`).
The platform layer of `control_manager.toml` comes from the model: mass 2.0 kg,
`motor_params a = 0.248316, b = 0` (throttle = a · sqrt(thrust per motor) + b with
kf = 2.534e-7 N/rpm², 0–8000 rpm), hover throttle 0.55.

## Frames (stage 4)

`transform_manager` publishes the inverted MRS tree with `uav1/fcu` as the root: on
`/tf`, `fcu → <estimator origin>`, `fcu → fixed_origin` and `fcu → world_origin` from
each `odom_main` sample, and `fcu → fcu_untilted` (fcu's heading without its tilt, which
`Se3Controller` and `ControlManager` look up every step) from each `hw_api/orientation`
sample; on `/tf_static`, `fixed_origin → local_origin` from the first
pose, and `fcu → rtk_antenna` (z 0.2 m), the static publisher of upstream's
`transform_manager.launch`. Upstream's 5 s wait for the antenna transform is a state
over steps; without it the step throws after 5 s. The native simulator has no GNSS, so
`world_origin` is the estimator's origin (upstream derives it from the first UTM fix and
the configured world origin). `estimation_manager`'s `Transformer` has no TF input: `/tf`
comes from `transform_manager`, which steps on its `odom_main`, and reading it would make
the two triggered cycles wait for each other.

`control_manager` checks a goto against the safety area of `mrs_uav_testing`'s
`default_world_config.yaml` (the ±50 m square in `world_origin`, z from 0.5 m to 15 m)
and the path from the last tracker command to it, with `mrs_lib::SafetyZone`.

## Edits to upstream (`// hypha port:`)

| File | Edits |
|---|---|
| `mrs_lib/.../attitude_converter.{h,cpp}` | the `tf2_ros`, `tf2_eigen`, tf (tf1) and `tf_conversions` includes removed, and the two `tf::Quaternion` overloads; the math is upstream's, on the vendored tf2 LinearMath |
| `mrs_lib/.../profiler.h` | replaced by a no-op with the same API (it only publishes monitoring messages); `profiler.cpp` not vendored |
| `mrs_lib/.../scope_timer.h` | includes roscpp used to bring |
| `mrs_lib/.../safety_zone/*.h` | `<eigen3/Eigen/Eigen>` → `<Eigen/Eigen>`; the unused `visualization_msgs` include removed |
| `mrs_uav_managers/.../estimation_manager/support.h` | `loadParamFile()` (the parameter server) removed |
| `mrs_uav_managers/.../{control,estimation}_manager/*_handlers.h` | `boost::function` → `std::function`; includes |
| `mrs_uav_state_estimators/.../ground_truth.{h,cpp}` | unused includes removed; the `sleep(1.0)` in `start()` removed (the manager retries) |
| `mrs_uav_controllers/src/se3_controller.cpp` | boost mutex, pointer and bind → std |
| `mrs_uav_controllers/include/mrs_uav_controllers/se3_controllerConfig.h` | written by hand from `cfg/se3_controller.cfg` |
| `mrs_uav_controllers/include/common.h`, `src/failsafe_controller.cpp` | includes |

## Not ported

MPC tracker and controller (binary-only solvers), constraint and gain managers,
autostart, trajectories, landing, `latlon_origin`, the frames `stable_origin`,
`amsl_origin`, `utm_origin` and the mapping origin, estimator switching,
the safety-area markers and services, and in the managers: joystick, RC, bumper,
parachute, escalating failsafe, the tilt and control-error safety timer, GPS and AGL
estimators.

## Licences

`upstream/` holds code of `mrs_lib`, `mrs_msgs`, `mrs_uav_managers`,
`mrs_uav_controllers`, `mrs_uav_trackers` and `mrs_uav_state_estimators` under their
BSD 3-Clause `LICENSE` (CTU Multi-Robot Systems group), and tf2's LinearMath headers
(`upstream/geometry2`, the Bullet zlib licence); `mrs_uav_testing` contributes
configuration values and the mission sequence. Nothing comes from
`mrs_multirotor_simulator` or `mrs_uav_hw_api`, which carry no licence file. The
commits are in each `upstream/<repository>/README.md`.
