# C++ API

A task guide to the C++ SDK: for each task, what to write and which program under
[examples/](../examples/README.md) does it. Each class and method is documented in its
public header (section 13 lists them). The run and its timing are in
[execution-model.md](execution-model.md), pitfalls in
[engineering-traps.md](engineering-traps.md), the C ABI in
[interface-specification.md](interface-specification.md).

A node includes `<hypha/hypha.h>`. Opt-in headers: `<hypha/cycle_transforms.h>` for
`hypha::Transforms` (section 10.1), `<hypha/transform_buffer.h>` for `hypha::TransformBuffer`
and `hypha::StaticTransformBroadcaster` (section 10.2), `<hypha/testing.h>` for the step
harness (section 5.4).

`hy` is a namespace alias of `hypha` (`hy::Node` is `hypha::Node`), declared in
`hypha_base/short_namespace.h`. Types, lookup, specializations (`hypha::QosOf`) and compiler
messages keep the name `hypha`. A program with its own global `hy` defines
`HYPHA_NO_SHORT_NAMESPACE` before the first hypha include. This guide writes `hypha::`.

| Task | Section | Example |
|---|---|---|
| A node that steps over its inputs at a rate | 1, 5.4 | `examples/speed`, `examples/scan_cycle` |
| A program with its own `main()` and a bounded run | 2 | `examples/sensor_fusion/src/probe_main.cpp` |
| Time, timers, simulated time | 3 | `examples/sim_twin` |
| Publish outside a cycle | 4 | `examples/sim_twin/src/sim_clock.cpp` |
| Handle each message as it arrives | 5, 5.2 | `examples/sensor_fusion/src/probe_main.cpp` |
| Optional and advisory inputs | 5.3 | `examples/scan_cycle` |
| Triggered cycles, a `/clock` driver, latched outputs, `failRun()` | 5.4 | `examples/mrs` |
| Ports built at run time | 5.5 | `examples/sensor_fusion` |
| Raw bytes and loans | 6 | `examples/large_payload`, `examples/bench` |
| A service policy for a topic | 7 | `examples/large_payload/inc/large_frame_qos.h` |
| Services | 8 | `examples/mrs/host/include/mrs_host/service.h` |
| Parameters and lists | 9 | `examples/sensor_fusion`, `examples/scan_cycle` |
| Transforms | 10 | `examples/mrs/managers/transform_manager.cpp`, `examples/mrs/host/include/mrs_lib/transformer.h` |
| Logging | 11 | `examples/speed`, `examples/scan_cycle` (`[log]` in `config/*.toml`) |
| Large payloads and sensor pipelines | 12 | `examples/large_payload`, `examples/bench` |

## 1. Quick start

A node whose work is one step over its inputs is a `hypha::CycleNode`: ports are members,
`step()` is a method, and the ports read their topics from the parameter file
(`examples/speed/src/speed_monitor.cpp` is a full one):

```cpp
#include <hypha/hypha.h>
#include <geometry_msgs/Pose.h>
#include <geometry_msgs/Twist.h>

class Follower : public hypha::CycleNode {
    hypha::Latest<geometry_msgs::Pose> pose{*this, "pose"};
    hypha::Output<geometry_msgs::Twist> command{*this, "command"};
    void step(const hypha::CycleTime&) override {
        if (pose.usable()) command.send(follow(*pose));   // follow() is the application's
    }
};

int main(int argc, char** argv) {
    return hypha::runNode<Follower>(argc, argv, "follower");
}
```

The parameter file is the program's one argument:

```toml
[cycle]
rate_hz = 50.0

[ports.inputs.pose]
topic = "/pose"

[ports.outputs.command]
topic = "/cmd_vel"
```

A node that handles each message as it arrives uses callback endpoints:

```cpp
#include <hypha/hypha.h>
#include <geometry_msgs/Pose.h>
#include <sensor_msgs/LaserScan.h>

void onScan(const hypha::Subscriber<sensor_msgs::LaserScan>::ConstPtr& scan);

int main() {
  hypha::Node node("my_node");
  auto pub = node.advertise<geometry_msgs::Pose>(hypha::Topic{"/pose"});
  auto sub = node.subscribe<sensor_msgs::LaserScan>(hypha::Topic{"/scan"}, onScan);
  const hypha::RunResult result = node.run();
  return result.ok() ? 0 : 1;
}
```

`hypha::Topic{...}` is a topic fixed in code; a bare string names a port table of the
parameter file (section 9). A default-constructed `hypha::Node` is a handle to the process
session, for code without the `Node` at hand (`hypha::Node().advertise<T>(...)`). It needs a
named `Node` first, or it throws `... failed: InvalidState: no session exists; create a named
hypha::Node first`.

In the object form the endpoints are members, and `hypha::bind` makes a member function the
callback. `hypha::bind<&C::onInput>(*this, index)` binds leading arguments, such as which of
several inputs a sample came from:

```cpp
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>
#include <sensor_msgs/Imu.h>

HYPHA_LOG_MODULE_INIT("attitude_node")

class AttitudeNode {
 public:
  AttitudeNode()
      : node_("attitude_node"),
        out_(node_.advertise<sensor_msgs::Imu>(hypha::Topic{"/attitude"})),
        in_(node_.subscribe<sensor_msgs::Imu>(
            hypha::Topic{"/imu"}, hypha::bind<&AttitudeNode::onImu>(*this))) {}

  hypha::RunResult run() {
    return this->node_.run();   // calls onImu on this thread for each message
  }

 private:
  void onImu(const hypha::Subscriber<sensor_msgs::Imu>::ConstPtr& imu) {
    if (!this->out_.publish(*imu)) {
      HYPHA_LOG_WARN_THROTTLE(1.0, "attitude not sent: {}",
                              this->out_.lastPublishStatus().name());
    }
  }

  hypha::Node node_;
  hypha::Publisher<sensor_msgs::Imu> out_;
  hypha::Subscriber<sensor_msgs::Imu> in_;
};
```

A `Node` must exist before an endpoint; `runNode` creates it for an application. A node that
reacts to messages and runs until stopped is a `hypha::ApplicationNode` whose `setup(Node&)`
creates its endpoints ([ros1-migration-guide.md](ros1-migration-guide.md#102-the-application));
the `main()` forms are for bounded probes and tools (section 2). SIGINT and SIGTERM make
`ok()` false and stop the run. `crates/hypha-cxx/test/test_quick_start_doc_examples.cpp` and
`test_cycle_doc_examples.cpp` (sections 5.4, 5.5, 10.1) compile these examples.

### 1.1 Messages

**Include paths.** Standard ROS packages install at their ROS 1 path, `<package>/<Name>.h`
(`#include <sensor_msgs/Imu.h>`). hypha's own messages and `ros_time.h` are in `hypha_msgs/`.
The SDK owns the directory of each standard package it ships; an application owns those of
the packages it declares, and declaring a standard package again from its own directory stops
the configure. A process cannot mix hypha messages with a ROS installation's message headers:
both define `std_msgs::Header` and the `ros::serialization` templates.

Up to release 0.2.0 the headers had a `hypha_msgs/` prefix (`<hypha_msgs/sensor_msgs/Imu.h>`).
It is gone, with no forwarding header: drop `hypha_msgs/` from each include
(`<hypha_msgs/hypha_msgs/NodeState.h>` becomes `<hypha_msgs/NodeState.h>`) and remove any
include directory added to reach the short path.

**`sensor_msgs` companions.** The SDK installs the header-only helpers ROS 1 ships beside the
messages, at the same paths:

- `<sensor_msgs/point_cloud2_iterator.h>`: `PointCloud2Modifier`, `PointCloud2Iterator<T>`,
  `PointCloud2ConstIterator<T>`;
- `<sensor_msgs/point_field_conversion.h>`: `readPointCloud2BufferValue`,
  `writePointCloud2BufferValue`, `pointFieldTypeAsType`, `typeAsPointFieldType` (include
  `<sensor_msgs/PointField.h>` or `PointCloud2.h` first, as upstream);
- `<sensor_msgs/image_encodings.h>`: the encoding names, `bitDepth`, `numChannels`, `isColor`,
  `isMono`, `isBayer`, `hasAlpha`;
- `<sensor_msgs/distortion_models.h>`.

They are ROS 1 `ros/common_msgs` tag 1.13.2 (BSD 3-Clause) in
`crates/hypha-cxx/upstream/common_msgs`, unchanged except for edits marked `hypha port:` (the
iterators take a view and access a field with `std::memcpy`). `vendor.toml` holds each file's
SHA-256; `python3 -I examples/autoware/tools/port.py verify` (run by `./check.sh`) checks them.

`*it` and `it[i]` of a `PointCloud2ConstIterator<T>` give a `T`. Of a `PointCloud2Iterator<T>`
they give a `hypha::UnalignedRef<T>` (`hypha/unaligned.h`), which converts to `T` and takes
`=`, `+=`, `++` and the like, so `*it = 1.0F`, `float x = *it`, `it[1]`, `++it` and
`it != it.end()` work as in ROS, at any alignment. What does not: a `T&` or `T*` to an element
(`float& r = *it`, `&*it`); `std::max(*it, 1.0F)` on a mutable iterator (write
`std::max<float>`); `printf("%f", *it)` (read into a `float` first). `auto v = *it` on a mutable
iterator is the reference, not a copy.

Left out: `fill_image.h`, and `point_cloud_conversion.h` (it needs the deprecated
`sensor_msgs/PointCloud`). There is no `camera_info_manager` (it needs a `NodeHandle`, a
service and a URL scheme): `hypha::readCameraInfo()` and `hypha::writeCameraInfo()` read and
write the calibration in TOML (section 9, *Camera calibration*).
`crates/hypha-cxx/test/test_sensor_msgs_companions.cpp` tests the companions.

**Your own messages.** One `hypha_generate_messages()` call per package generates its `.msg`
and `.srv` files into an INTERFACE library of the package's name; linking it brings the
include path and build order (`examples/mrs/CMakeLists.txt`):

```cmake
find_package(hypha CONFIG REQUIRED)
hypha_generate_messages(PACKAGE app_msgs MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg"
                        DEPENDS std_msgs)
target_link_libraries(app PRIVATE hypha::sdk app_msgs)
```

`DEPENDS` names a standard package or one an earlier call declared. Never copy or regenerate a
standard package.

**Adding to an SDK package.** A call that names an SDK package and gives definitions of its own
adds to that package. It reads what the SDK ships, so a `.srv` can use `sensor_msgs/CameraInfo`
without `DEPENDS`, and emits only the new definitions:

```cmake
hypha_generate_messages(PACKAGE sensor_msgs
                        SRV_DIR "${CMAKE_CURRENT_SOURCE_DIR}/srv/sensor_msgs")   # SetFocusRegion.srv
target_link_libraries(app PRIVATE hypha::sdk sensor_msgs_additions)
```

- Headers land at `<OUT_DIR>/sensor_msgs/SetFocusRegion.h`, beside the SDK's, not among them;
  the install lists `HYPHA_GENERATED_HEADERS` of the target.
- The target is `<package>_additions`; `<package>` is the SDK's.
- A name the package already has (a message, a service, or a half of one such as
  `SetCameraInfoRequest`) is refused by name: ROS definitions are not redefined.
- `PACKAGE` in its own `DEPENDS` is refused. One call adds to a package. A later `DEPENDS` on
  the package names the SDK's definitions only; `hypha_generate_ros2_headers()` covers both.
- `test/package_consumer` adds to `sensor_msgs` from an installed prefix.

**What a message type gives.** Every message `M` has the pointer typedefs of both ROS
generations (`M::Ptr`, `M::ConstPtr`, `M::SharedPtr`, `M::ConstSharedPtr`, `M::UniquePtr`) and
`M_<Allocator>` as in ROS 2. Each field `f` has `M::_f_type` and ROS 2's setter
`M& set__f(const M::_f_type&)`. `==` compares field by field, `time` and `duration` included.
Types, constants and the wire format follow ROS 1; engineering-traps.md lists the surprises
(`byte` is signed, an out-of-range constant fails generation, no big-endian target).

**ROS 2 names.** Code written for ROS 2 includes `<sensor_msgs/msg/point_cloud2.hpp>` and names
`sensor_msgs::msg::PointCloud2`. One call generates those names for the packages you use;
hypha-cxx installs none:

```cmake
hypha_generate_ros2_headers(app_ros2_headers PACKAGES std_msgs sensor_msgs app_msgs)
target_link_libraries(app PRIVATE hypha::sdk app_msgs app_ros2_headers)
```

- Each message gets `<package>/msg/<name>.hpp` and each service `<package>/srv/<name>.hpp`,
  spelled as rosidl does (`Vector3Stamped` is `vector3_stamped.hpp`).
- The header includes `<package/Name.h>` and declares `package::msg` (and `package::srv`) as
  names of `package`, for its own package and every package its fields reach. A service adds
  `Name_Request` and `Name_Response`. A type that reaches `time` or `duration` also names
  `builtin_interfaces::msg::Time` and `Duration`.
- The type is still the ROS 1 type: fields keep ROS 1 names (`header.seq`, `stamp.secs`, not
  `stamp.sec`), so code that reads a ROS 2 field name fails to compile rather than read another
  value. `test/package_consumer/ros2_style.cpp` compiles against these names alone.
- hypha-cxx installs the ROS 2 spellings of the companions (`<sensor_msgs/point_cloud2_iterator.hpp>`,
  `point_field_conversion.hpp`, `image_encodings.hpp`, `distortion_models.hpp`). Each includes
  the `.h`, and the first two also `<sensor_msgs/msg/point_cloud2.hpp>` and `point_field.hpp`
  when generated. The classes stay in `sensor_msgs` as in both ROS generations
  (`sensor_msgs::PointCloud2Iterator<float>`), so ROS 2 code compiles unchanged
  (`test_sensor_msgs_ros2_names.cpp`).

## 2. Node lifecycle

An application derives from `hypha::ApplicationNode` or `hypha::CycleNode` and returns
`hypha::runNode<App>(argc, argv, "name")` from `main` (`examples/speed`; phases and exit codes
in section 5.4). A program with its own `main()` constructs a `hypha::Node`, creates its
endpoints and calls `run()` or `runFor()` (`sensor_fusion/src/probe_main.cpp`). Reference:
`hypha/node.h` (`Node`, name rules), `hypha/raw.h` (`RunResult`), `hypha/application_node.h`,
`hypha/topic.h` (`~/x`); the run is execution-model.md §4.

- **Node report.** `runNode` publishes `hypha::introspect()` latched on `~/introspection` and
  `hypha::nodeState()` on `~/state` every second (section 9.1). A program with its own `main()`
  gets neither, nor the refusal of unread parameters: it publishes both itself and checks
  `config().unreadKeys()`.
- **Input state.** The bindings of one topic give one input with the worst verdict. An
  `Arrivals` input without a new sample is still usable, and `setIgnored(true)` makes an input
  ok, so `inputsOk()` and the state agree. Each change is logged (section 5.4, "Input
  supervision").
- A publisher of another type on the node's own `~/state` or `~/introspection` is refused with
  `InvalidTopic` (`~/state carries hypha_msgs/NodeState, not <type>`); under `runNode` the
  start exits 70.
- `HYPHA_DOMAIN` separates whole systems on one host (interface-specification.md).

### 2.1 Library version and ABI revision

The root file `abi.revision` holds the revision. Each build reads it into `HYPHA_ABI_REVISION`
(`hypha/hypha_ffi.h`), and the library reports its own from `hypha_abi_revision()`. Before any
other ABI call the SDK compares them exactly. A mismatch refuses the start: `Node` and
`ConfigContext` throw `InvalidState`, `runNode` returns 78 before it reads the parameter file,
the Rust crate refuses `BusNode`, and standard error (not the `[log]` sink) gets
`hypha ABI revision mismatch: SDK expects 1, library reports 2`. Calls that cannot fail, such as
the clock, skip the check; a C ABI caller compares the values itself. The library file name
carries the revision (getting-started.md), so a binary built against another revision does not
find its library: rebuild every consumer.

### 2.2 Waking a node from outside the framework

There is no payload-free event channel. A wake-up from outside (an interrupt thread, another
process) is a small message, such as a `std_msgs/UInt8` code, read by a callback subscriber or a
triggered cycle; every publisher notifies. Latch it for late starters.

### 2.3 Cleaning up

- **One session.** Every `Node` of a process joins one session. The last one destroyed leaves
  the bus and destroys every cycle of the session, whatever the creation order (#97). A
  default-constructed `Node` joins nothing and does not count (`hypha/node.h`).
- **One name, one file.** A second `Node` keeps the first name and shares the process's parameter
  file (section 9): it names no file or the same one; another file throws, naming both (#208).
- **After the session left**, `publish()` returns false with `lastPublishStatus()` `InvalidState`,
  as the C ABI's loan and send do once the node is destroyed.
- **Errors.** A start-up error throws from `Node`, `advertise`, `subscribe` or `timer`, naming the
  port or topic and status (`hypha: ports.outputs.command failed: NotAvailable`). Bad bus data is
  counted, logged at a bounded rate and dropped, never fatal. No exception crosses the C ABI;
  `hypha::require()` turns a `std::expected` into one (`hypha/status.h`).

A late subscriber of a latched topic gets its sample from the publisher's housekeeping, so the
publishing process must run its executor (execution-model.md §3). A process with its own loop
runs bounded turns:

```cpp
hypha::Node node("sample_source");
auto publisher = node.advertise<std_msgs::Float64>(hypha::Topic{"/samples"});
while (hypha::ok()) {
    const std_msgs::Float64 sample{1.0};
    if (!publisher.publish(sample)) {
        return 1;
    }
    const auto result = node.runFor(hypha::Duration::fromMillis(10));
    if (!result.ok()) {
        return 1;
    }
}
```

A timer with `node.run()` serves it too (`examples/sim_twin/src/sim_clock.cpp`). A schema needs
no executor: the publisher writes its schema record when it is created.

### 2.4 Metrics

`hypha/metrics.h` reads the counters of interface-specification.md "Metrics" (no C++ wrapper for
`integrity.*` and `loan.*`). On host time a `node.last_sweep_end_nanos` that stops moving means no
thread runs the node's `WaitSet`. On simulated time it follows the replay clock, so alone it shows
nothing.

### 2.5 A worker thread ends at the node's stop

A thread the application owns (a driver's grab loop) takes the node's stop from
`this->stopToken()`, a `std::stop_token` that `runNode` stops when the run ends, whatever ended
it: `hypha::requestStop()`, SIGINT or SIGTERM, `failRun()`, a refused start-up. It stops before
the application's destructor runs, so the destructor only joins. `hypha/worker.h` replaces
`ros::ok()`, `ros::Duration::sleep()` and `ros::Rate`:

```cpp
void grabLoop(std::stop_token stop)             // this->stopToken(), copied into the thread
{
    hypha::Rate rate(30.0, stop);               // ros::Rate rate(30)
    do {
        grabAndPublish();
    } while (rate.sleep());                     // false: the node stopped
    // after a failed grab: if (!hypha::sleepFor(stop, std::chrono::milliseconds(100))) return;
}
```

- `sleepFor(stop, span)` and `sleepUntil(stop, steadyDeadline)` return true when the whole time
  passed and false when the stop ended the sleep or had already come. They are `[[nodiscard]]`
  because a stopped sleep returns at once every time, so a loop that ignores the result spins.
  `span` is a `std::chrono` duration or a `hypha::Duration`.
- `Rate(hz, stop)` or `Rate(Duration, stop)` keeps a grid: the first tick is one period after
  construction; work shortens the wait and never moves the grid. `sleep()` returns true after the
  tick and false at the stop (not, as in ROS, at an overrun). A pass past a tick returns at once;
  a pass past whole periods skips them without a burst and adds them to `missedCount()`, as
  `Timer::missedCount()` does. `reset()` restarts the grid. A rate outside 1 ns to 24 h throws
  `std::invalid_argument`.
- This is host steady-clock time, also on simulated time: a thread has no wait for `/clock`. A
  loop that must follow simulated time is a timer or a cycle.
- The token also ends a `std::condition_variable_any::wait(lock, stop, predicate)` on the worker's
  own queue, and a `std::stop_callback` can wake any other blocking wait.
- Reference: `hypha/worker.h`, `hypha/application_node.h`; thread rules in
  [ros1-migration-guide.md](ros1-migration-guide.md#106-threads).

### 2.6 A hand-over from threads the node does not own

A vendor SDK that calls back on its own threads (a lidar's cloud, IMU and fault callbacks), or a
blocking device read, must not publish from there: `Publisher::publish()` runs on one thread at a
time. `hypha/handover.h` is the bounded queue between those threads and the one worker that owns
the publishers, as ROS 1's callback queue was:

```cpp
hypha::Handover<Cloud, Imu> queue(8U, this->stopToken());          // 8 of each kind
hypha::Handover<Cloud, Imu> queue({2U, 400U}, this->stopToken());  // 2 clouds, 400 samples

queue.push(toRos(cloud));                       // any thread; never waits
while (auto item = queue.pop()) {               // the worker; ends at the node's stop
    std::visit([this](const auto& m) { this->send(m); }, *item);
}
```

- A kind at its depth drops its oldest message. Every drop is counted (`dropped()` for all kinds,
  `dropped<Kind>()`), `push()` still returns true, and the log warns at most once a second. Each
  kind has its own depth, so a burst of one never displaces another.
- `pop()` waits and returns messages in push order (one thread's order is kept). `popFor(timeout)`
  also returns empty at the timeout; `stopped()` tells that from the stop. After the stop both
  return what is queued, then empty: the worker drains and ends, and the destructor only joins.
- The stop is the token given at construction, `this->stopToken()` (section 2.5; valid in the
  constructor's initializer list). A default token never stops a wait.
- A `Handover` is neither copyable nor movable; move-only kinds are fine. Each kind appears once:
  send two kinds of one type as two wrapper types.
- The drop count is not in `~/state`; that needs a `NodeState` field and counter registration
  (#233).
- Reference: `hypha/handover.h`; thread rules in
  [ros1-migration-guide.md](ros1-migration-guide.md#106-threads).

### 2.7 Application status: `DiagnosticUpdater`

`~/state` judges a node's inputs. Conditions only the application knows (device absent, fix lost,
rate low) are named checks, written as in ROS's `diagnostic_updater`: a status task with a name, a
level and a message. A check declared once reaches two readers:

- **`~/state`** gets one row per check (`NodeState.checks`); the node's state follows it (below)
  and `hypha top` shows it.
- **`/diagnostics`** gets the same checks as one `diagnostic_msgs/DiagnosticArray`, which a ROS
  `diagnostic_aggregator` reads, when the updater has a sink.

`hypha/diagnostic_updater.h` (the updater and tasks), `hypha/diagnostic_status.h` (the status a
task fills) and `hypha/diagnostic_functions.h` (rate and stamp checks) are not in
`hypha/hypha.h`. Why one mechanism and not two: [features.md](features.md) section 4,
"Application status".

The updater is a tier-2 companion (`crates/hypha-cxx/companions/`). It reports through the public
`hypha::reportChecks(owner, checks)` of `hypha/introspection.h`, which a driver may also call
directly. `owner` keys one reporter's rows: a second report replaces them, an empty one removes
them. Each `hypha::HealthCheck` has a name (1 to 128 bytes), a level 0 to 3 (`OK`, `WARN`,
`ERROR`, `STALE`) and a message. It returns `InvalidArgument`, or `TooLarge` past 64 checks in
all, and then changes nothing.

A node with no aggregator to feed (most drivers) builds the updater without a sink:

```cpp
#include <hypha/hypha.h>
#include <hypha/diagnostic_updater.h>

class ArenaCamera final : public hypha::ApplicationNode
{
  public:
    ArenaCamera()
    {
        updater_.add("Device", this, &ArenaCamera::diagnoseDevice);
    }

    void setup(hypha::Node& node) override
    {
        // The timer is the period; its task runs on the run thread.
        timer_ = node.timer(hypha::Duration::fromSeconds(1.0), [this] { updater_.forceUpdate(); });
    }

    void started(hypha::Node&) override
    {
        updater_.forceUpdate();  // the first row at once, not after a period
        // ... start the worker that sets connected_ and noImage_
    }

  private:
    // Reads what the worker publishes; never waits for the device.
    void diagnoseDevice(hypha::DiagnosticStatusWrapper& status)
    {
        if (connected_) {
            status.summary(diagnostic_msgs::DiagnosticStatus::OK, "camera connected");
        } else {
            status.summary(diagnostic_msgs::DiagnosticStatus::ERROR, "camera absent");
        }
        status.add("grabs without image", noImage_.load());
    }

    hypha::DiagnosticUpdater updater_;
    hypha::Timer             timer_;
    std::atomic<bool>        connected_{false};
    std::atomic<uint64_t>    noImage_{0};
};
```

With the camera absent, `~/state` says `DEGRADED` with the row `Device: ERROR camera absent`, and
`hypha top` shows it under the node. Once the worker sets `connected_`, the next update says `OK`.

With a sink (an `Output<DiagnosticArray>` or a `Publisher<DiagnosticArray>`), the tasks also go
out on `/diagnostics`:

```cpp
#include <hypha/hypha.h>
#include <hypha/diagnostic_functions.h>
#include <hypha/diagnostic_updater.h>

class Driver final : public hypha::CycleNode
{
  public:
    // [ports.outputs.diagnostics] topic = "/diagnostics"
    Output<diagnostic_msgs::DiagnosticArray> diagnostics{*this, "diagnostics"};
    hypha::DiagnosticUpdater                 updater{diagnostics};

    Driver()
    {
        updater.setHardwareID("lidar");
        updater.setPeriod(hypha::config().optional<double>("diagnostic_period", 1.0));
        updater.add("Connection", this, &Driver::diagnoseConnection);
    }

    void step(const hypha::CycleTime&) override
    {
        updater.update();
    }

    void diagnoseConnection(hypha::DiagnosticStatusWrapper& status)
    {
        if (connected) {
            status.summary(diagnostic_msgs::DiagnosticStatus::OK, "connected");
        } else {
            status.summary(diagnostic_msgs::DiagnosticStatus::ERROR, "device absent");
        }
        status.add("peer", peer);
    }
    // ...
};
```

- **Sink.** An `Output<DiagnosticArray>` sends in the step that calls `update()`; a
  `Publisher<DiagnosticArray>` sends at once, and a refused publish throws `std::runtime_error`
  with the status. The topic is a line of the parameter file, as for every port. An
  `ApplicationNode` calls `forceUpdate()` from `node.timer(...)`, whose period is then the period.
  `DiagnosticUpdater updater;` has no sink and reports to `~/state` only.
- **`~/state` rows.** Each update replaces the updater's rows in `NodeState.checks`: the task's
  name (without the node's, which `NodeState.node` has), its level (`OK`, `WARN`, `ERROR`,
  `STALE`) and its message. Key/value pairs stay on `/diagnostics`, so put a number that matters
  in the message. A message over 256 bytes is cut and ends in `...`. At most 64 checks per node,
  names of 1 to 128 bytes; an update that would pass them throws `std::runtime_error` (the array
  still goes out, and no row of that update is kept). The state goes out every second, so a row is
  as old as the last update: shorten the period for a fault that must not wait. A destroyed
  updater takes its rows away; `removeByName` drops a row at the next update. Without a `Node` (a
  test of a task) nothing is reported.
- **Level to state.** `ERROR` and `STALE` make the node `DEGRADED`, as a failing required input
  does (`WAITING` still wins while an input never arrived). `WARN` and `OK` change nothing else. A
  check that cannot say is not one that passed, so `STALE` counts as `ERROR`.
- **Failures are statuses.** A task that sets nothing reports ERROR "No message was set". A task
  that throws reports ERROR "Exception in diagnostic task: <what>", without the values it added. A
  level outside OK, WARN, ERROR, STALE reports ERROR "invalid level n: ...". The other tasks still
  run. A task that reports STALE reports it as itself.
- **Cost.** Tasks run in the thread that calls `update()` or `forceUpdate()` (the step, or the
  timer on the run thread), once a period. They read what a worker publishes and never wait on a
  device. The report is one call that copies at most 64 short rows under a lock the state reader
  holds as briefly. Nothing runs between two updates.
- **Names.** Each status is named `<node name>: <task name>` and carries the `hardware_id`
  ("unspecified" until `setHardwareID`), as ROS writes them, so a `GenericAnalyzer` matches. The
  array's `header.stamp` is the update's framework time.
- **Time.** `update()` runs the tasks when a period has passed (default 1 s). The first call only
  starts the period, as ROS's timer does; `forceUpdate()` runs at once. `update(Time)` takes the
  time (`CycleTime::now`); `update()` reads `hypha::now()`, which in a step is the step's time, so
  replays and simulated clocks decide it. Time that goes back restarts the period.
- **Tasks.** `add(name, function)`, `add(name, object, &Class::method)`, `add(task)` (a
  `DiagnosticTask`, `FunctionDiagnosticTask` or `CompositeDiagnosticTask`, which must outlive the
  updater), `removeByName`, `broadcast(level, message)`. `DiagnosticStatusWrapper` has `summary`,
  `summaryf`, `mergeSummary`, `clearSummary`, `add` (a bool reads `True` or `False`), `addf`,
  `clear`, and `now()`, the update's time.
- **Rate and stamp checks.** `FrequencyStatus` (`tick()` per event; ERROR with no event in the
  window, WARN outside `[min * (1 - tolerance), max * (1 + tolerance)]`); `TimeStampStatus`
  (`tick(stamp)`: each stamp's age since the last update); `TopicDiagnostic` and
  `HeaderlessTopicDiagnostic` (both, added to the updater: `scan.tick(msg.header.stamp)` before
  `send`). `FrequencyStatusParam` takes pointers to the limits, as ROS does, so a parameter may
  change them; they must outlive it. `DiagnosedPublisher` has no counterpart.
- **Threads.** One thread calls the updater; a task's `tick()` may come from another.
- **Policy.** `/diagnostics` has many publishers, so the type maps to `hypha::diagnosticsQos` (64
  publishers, 8 subscribers, depth 128, messages up to 64 KiB), as `/tf` does. An aggregator's
  input needs a `perStep` for the burst of all publishers (section 5.4).

Test with `testing::StepHarness`: `harness.sent(node.diagnostics)` holds the step's array, and
`update(Time)` or a `CycleTime` drives the clock (`crates/hypha-cxx/test/test_diagnostic_updater.cpp`).

## 3. Time and clock

Pace periodic work with a cycle (section 5.4) or `Node::timer()`; there is no blocking wait.
`examples/sim_twin` runs cycles and a timer on the `/clock` of its `sim_clock.cpp`. Reference:
`hypha_base/time.h`, `hypha_base/clock.h`, `hypha/message_stamp.h`, `hypha/cycle.h`
(`CycleTime`, `Hz`); simulated time is execution-model.md §11.

- **`/clock`.** The framework `/clock` subscriber wakes on every `/clock` publisher's
  notification. `/clock` going back ends a cycle's or timer's run with `ClockRewound`, and
  `runNode` restarts the process.
- **Timers.** `cancel()` stops calls until `reset()`; a canceled timer counts no missed period
  and costs no wake-up. `reset()` puts the next call one period ahead. `state()` returns a
  `TimerStatus`: `canceled`, `period`, and `untilDue` (empty while canceled), all on the timer's
  clock (framework time on simulated time, else host monotonic). A cancel or reset from a callback
  also holds back a call of that timer still due later in the same turn; from another thread
  during a run each returns `InvalidState`. A callback may create a timer on host time or on the
  node's own simulated clock; it joins before the run next waits. On a simulated clock the node
  does not own, `Node::timer()` throws. Reference: `hypha/node.h` (`Timer`, `TimerStatus`).
- **Replay.** A plain subscription or timer takes no part in the lockstep: it runs as its sample
  arrives, `hypha::now()` is the clock, and a timer fires once per turn with its missed count
  (execution-model.md §11.2, #153). Only cycles are held, ordered and reported.
- **Seconds as `double` must be finite.** `Duration::fromSeconds`, `Time::fromSeconds` and
  `Duration * double` throw `std::invalid_argument` for NaN or infinity (a compile error in a
  constant expression), and clamp finite input outside the int64 nanosecond range. Where a throw is
  not allowed, such as a step, `Duration::tryFromSeconds` returns an empty `std::optional` (#126).
- **Stamps.** There is no implicit `Time` to `RosTime` conversion. Never stamp with `steadyNow()`.
  Check a time computed from an input once, where it enters:

```cpp
status.header.stamp = time.stamp;                      // in a step
status.header.stamp = hypha::toRosTime(hypha::now());  // in a callback
status.header.stamp = hypha::rosTimeFromSeconds(packetSeconds);  // or ...FromNanos(n)
```

On simulated time `hypha::now()` is a placeholder zero until the clock has a value.
`hypha::isClockValid()` says whether a stamp from it is a time: true on host time, and on
simulated time after the first `/clock` sample, `setSimTime` or `enableSimTime`. The run reads
`/clock`, so a constructor or `setup()` sees false.

## 4. Publishing

Outside a cycle, `node.advertise<M>(port | hypha::Topic{...}, options...)` creates a
`hypha::Publisher<M>` (`examples/sim_twin/src/sim_clock.cpp`; in a cycle, an `Output`). `node`
may be `hypha::Node()` once a named `Node` exists (section 1); before that `advertise` throws
`... InvalidState: no session exists`. Reference: `hypha/node.h`, `hypha/publisher.h`,
`hypha/topic.h`, `hypha/options.h`.

- **`publish()` is `[[nodiscard]]`**: a refused send is a result, never a silent loss.
  `publishOrWarn(msg)` is for a node with no reaction to a refusal: it returns the same `bool`
  and warns with the topic and status at most once a second per publisher (copies share one
  count; a disabled publisher is silent). A cycle `Output::send` needs neither.
- **Size.** A fixed-size message is bounded to its encoded size. Any other is growable unless
  `hypha::maxPayloadDynamic(bytes)` or the policy (section 7) names its largest message; a larger
  one is `TooLarge`. A growable publisher reserves its pool at its first slot size, 4096 bytes per
  sample: give small messages the policy's `initialPayload` (a hint, not a bound) to keep the
  topic's shared memory small (engineering-traps.md). An image or cloud names its bound and a
  policy; section 7.1 sizes its pool, and `examples/large_payload` is the raw version.
- **`subscriberCount()`** counts the subscribers of every node on the machine at that instant. It
  is not delivery. It returns `std::optional<size_t>`: empty means no count is available (a
  disabled publisher, or a transport that cannot count), `0` means no subscriber now. Skip costly
  work only on a counted `0`. ROS 1 mapping:
  [ros1-migration-guide.md](ros1-migration-guide.md#104-time-status-and-logging).

**Writing a large message in place.** `loan(head, capacityBytes)` reserves a slot for `head`'s
small fields and returns a `Loan<M>` whose `data()` is the loaned field's bytes in shared memory
(`hypha/loan.h`). The loaned field is the message's single top-level array of numbers, with only
fixed-size numbers after it. The generator marks it (`hypha::message_traits::LoanedField`) in 15
standard types: `Image`, `CompressedImage`, `PointCloud2`, `OccupancyGrid` and the eleven
`*MultiArray` types. A message with two such arrays (`LaserScan`, `JointState`, `Joy`) has none,
and `loan()` on it does not compile ("no single top-level array of numbers").
`publish(std::move(loan), usedBytes)` encodes the small fields around the first `usedBytes` (may be
fewer than reserved) and sends exactly what `publish(msg)` would, with the next `header.seq`.

- The lengths of `head`'s strings and arrays (`frame_id`, `encoding`, `fields`) are fixed at the
  loan; their contents and the numbers (`width`, a stamp) may change through `loan.message()`
  until the publish.
- `data()` is not aligned: the ROS 1 encoding has no padding (a `Float64MultiArray`'s numbers may
  start at 4 modulo 8). Write with `std::memcpy` or `loan.put(i, value)` (copies a trivially
  copyable `value` to element `i`); never through a cast `Point*`, nor with aligned SIMD stores.
- A span from `data()` is dead after the publish. Writing through it changes a sent frame, which
  subscribers refuse as corrupt.
- Nothing waits, and a refused loan loses no frame: ask again later. `PoolExhausted` is a full
  pool; `LoanLimit` means this publisher's two loans are both held. (For `publish(msg)` the same
  refusals drop the message, and subscribers count it lost.)
- A loan dropped unsent returns its slot and takes no `header.seq`. Another publisher's loan, a
  moved-from loan (`capacity()` 0), a size not in whole elements or past `capacity()`, or a
  changed length is `InvalidArgument` at the publish: nothing is sent, no `header.seq` taken.
- The CRC is computed at the publish, over the bytes in the slot.

```cpp
sensor_msgs::PointCloud2 head = cloudHead();           // frame_id, fields, point_step
auto loan = points.loan(head, maxPoints * sizeof(CloudPoint));
if (!loan) { return; }               // PoolExhausted, LoanLimit, TooLarge: no frame lost
size_t count = 0;
for (const auto& p : decoded) {
    loan->put(count++, CloudPoint{p.x, p.y, p.z, p.intensity, p.ring, p.timestamp});
}
loan->message().width = count;
loan->message().row_step = count * sizeof(CloudPoint);
(void)points.publish(std::move(*loan), count * sizeof(CloudPoint));
```

**In a cycle step** the same works on an output: `Output<M>::loan(head, capacityBytes)` reserves a
slot of this step, and `output.send(std::move(*loan), usedBytes)` registers it, shrunk. It goes out
after a successful step, in send order, with the bytes `send(const M&)` would send
(`hypha/cycle_output.h`). A step may hold several loans of several outputs and send other messages
meanwhile. A loan counts against `perStep()` from the loan; dropped unsent it gives back its slot
and count. A loan kept past its step returns its slot at the step's end, and sending it in a later
step throws `std::logic_error`. A refused loan (`PoolExhausted`, `LoanLimit`, `ResourceLimit` past
`perStep()`, `TooLarge`) does not fail the step. In a `testing::StepHarness` the loan is memory of
its own and `sent(output)` holds the message.

```cpp
void step(const hypha::CycleTime&) override {
    for (const hypha::ViewPtr<PointCloud2>& cloud : this->input.samples()) {
        auto loan = this->output.loan(headOf(*cloud), cloud->data.size());
        if (!loan) { continue; }                       // no slot free: nothing lost
        const size_t used = downsample(*cloud, loan->data());   // writes the points
        this->output.send(std::move(*loan), used);
    }
}
```

## 5. Subscribing

To react to each message as it arrives, create a subscriber with a callback,
`node.subscribe<M>(port | hypha::Topic{"/x"}, callback, options...)`, and run the node
(`examples/sensor_fusion/src/probe_main.cpp` uses two named functors and `runFor()`). A bare string
names `[ports.inputs.<port>]` (section 9). Options: a `hypha::Qos` (section 7) and, for a port,
`hypha::optionalTopic` (section 5.3). The callback is optional. A literal topic wakes the run per
sample; a `*` pattern is polled every 1 ms (execution-model.md §7-8). A cycle input port warns about
a missing publisher like a subscriber, once per topic. Literal topic versus port name, and porting a
ROS 1 node to an `ApplicationNode`:
[ros1-migration-guide.md](ros1-migration-guide.md#103-literal-topic-or-port-name). Reference:
`hypha/subscriber.h` (type check, patterns, the no-publisher warning), `hypha/node.h`.

**Several types on one topic.** A topic carries one stream per message type and version (its MD5).
`subscribe<A>("/x")` and `subscribe<B>("/x")`, a `Latest<A>` and an `Arrivals<B>` on one port name,
or two `Output`s of two types on one name are independent: each has its own stream, verdict,
`silence_ms`, queue and policy, and a frame of the other type is neither delivered nor counted as a
fault. The stream is the MD5, not the type name: two names of one definition share a stream. This is
how a message is migrated: version 1 and 2 of `pkg::Odometry` run on one topic while the nodes are
rebuilt one by one.

Another version on the topic is shown, never hidden:

- `sub.metrics().otherSchemas` counts the other MD5s that a live publisher sends on the topic. The
  `~/state` row of a cycle input names them (`RequiredInput.other_schemas`: schema hash, type and
  MD5, one entry per live other version).
- Housekeeping refreshes them after a graph event and at least once a second while the node runs,
  so the fact is at most about a second old. Before the first refresh `otherSchemas` is
  `std::nullopt` and `other_schemas_checked` false. A peer still on the old version shows; a killed
  one stops showing.
- The verdict says what the data says: an input whose version nobody publishes is `NeverArrived` if
  it never had a publisher, `Stale` after its `silence_ms` once it had one, and `Usable` on its last
  value when it has no limit.
- While its own stream has no publisher, a subscriber logs after 5 s, again when the published
  versions change, at most once in 10 s:

```text
hypha: topic '/odom' has publishers, none of type pkg/Odometry (581e4b1b): published are pkg/Odometry (f79835de)
```

The `~/state` row of an input names its `data_type` and `md5`, so two types of one topic are two
rows ([interface-specification.md](interface-specification.md#streams)).

### 5.0 Reading a large message in place

Subscribe to `hypha::View<M>` instead of `M` (`hypha/view.h`) to read a camera frame or a lidar
cloud in the publisher's shared-memory slot, after the receive's one CRC check, with no copy.
`View<M>` has the decoded fields of `M`, with its byte array (`Image::data`, `PointCloud2::data`) as
a `std::span<const uint8_t>` of the slot. The generator emits a view for every message whose loaned
field (section 4) is a byte array: `Image`, `CompressedImage`, `PointCloud2`, `OccupancyGrid`,
`UInt8MultiArray`, `Int8MultiArray`, `ByteMultiArray`, and an application's own.
`hypha::ViewPtr<M>` owns a view as `M::ConstPtr` owns a message, so ported code reads unchanged:
`msg->width`, `msg->data.size()`, `msg->data[i]`,
`sensor_msgs::PointCloud2ConstIterator<float> x(*msg, "x")`,
`sensor_msgs::image_encodings::bitDepth(msg->encoding)`.

```cpp
void onCloud(const hypha::ViewPtr<sensor_msgs::PointCloud2>& msg) {
    sensor_msgs::PointCloud2ConstIterator<float> x(*msg, "x");   // reads the slot
    for (; x != x.end(); ++x) { /* ... */ }
    this->latest_ = msg;               // keeps the slot (one per subscriber), no copy
}
auto sub = node.subscribe<hypha::View<sensor_msgs::PointCloud2>>("points", onCloud);
```

A cycle input reads views too: `Arrivals<View<M>>::samples()` gives a `ViewPtr<M>` per sample, and
`Latest<View<M>>` gives a `View<M>` (`*port`, `port->width`) and its ViewPtr (`port.sample()`). On
host time the input keeps a message of 64 KiB or more in the subscriber's slot. On simulated time,
or for a smaller message, the step reads the input's copy, in place all the same. A
`std::shared_ptr<const M>` converts to a `ViewPtr<M>` over the message's own array, so a test that
passes a message to code that now takes a view still compiles, and a `testing::StepHarness` feeds a
view port with messages (`deliver(port, std::vector<M>{...})`).

The lifetime is the ViewPtr's, and it is bounded:

- Copies share one view; the bytes live while any copy lives. A `std::span` or pointer taken from a
  view lives no longer than its ViewPtr.
- A copy held past its callback keeps the slot, within the subscriber's loan budget of one slot
  (`metrics().loanBudget`, `loanHeld`; one stays free for the next delivery). So `latest_ = msg;` in
  each callback keeps every frame in place. Past the budget the held copy's bytes are copied once
  at that return, logged (at most once in 10 s) and counted in `metrics().loanRefused`. Nothing is
  dropped and nothing dangles.
- A copy held past a cycle step owns a copy of its bytes from there, logged. A cycle input whose
  step takes fewer samples than arrive copies the oldest out of its slot to receive the next, logged
  with the count.
- `hypha::toMessage(*msg)` copies a view into an `M`, for code that takes `const M&`.
- The bytes are where the ROS 1 encoding puts them, not aligned. The `PointCloud2` iterators use
  `std::memcpy`, so they are safe on any cloud (section 1.1). Code that takes its own pointer into
  `data` must read multi-byte numbers with `std::memcpy`:
  `reinterpret_cast<const float*>(msg->data.data())` is undefined behaviour (UBSan reports a
  misaligned access).
- Do not read a ViewPtr on another thread before its callback or step returns: the return may move
  the bytes into a copy. A view subscriber keeps no `latestSample()` (it would hold the slot).

### 5.1 Freshness and the latest value

Every frame carries its publisher's host-monotonic send time. Only cycle inputs judge freshness, at
capture, below the C ABI, against each port's `silence_ms` ("Input supervision"). One topic can
serve ports with different limits, and a callback subscriber never discards a frame for its age
(execution-model.md §6). `examples/speed/src/speed_monitor.cpp` acts on the verdict. Transport time
is not source time: a value that needs sensor time carries it in its payload, such as a ROS
`Header`. Arrival period and jitter do not decide validity; a tool that needs them measures the
deliveries.

A step reads a sample's origin from its frame header: `port.sentAt()` (host-monotonic send time),
`port.sequence()` (its publisher's send sequence) and `port.publisherId()` (the publisher instance).
On a `Latest` port they describe the held value and are empty unless `has()`. On an `Arrivals` port
they take an index into `samples()` and throw `std::out_of_range` past the end. The sequence counts
one publisher's frames and wraps. The library has already refused repeated or older frames and
counted lost ones ([communication-integrity.md](communication-integrity.md)), so a step needs no
sequence check. In a `StepHarness` sequence and publisher read zero; the send time reads zero after
`set()` and the given time after `deliver()`.

### 5.2 Two ways to read a subscription

- **Callback.** One call per delivered sample, on the run thread. The run drains each queue and
  calls back once per surviving sample, so the queue depth bounds the backlog. Use a named functor
  (`probe_main.cpp`). A slow callback is counted and warned about (execution-model.md §5).
- **Cycle.** One frozen capture of every input per step, then the sent outputs. Use
  `hypha::CycleNode` (section 5.4) for a class, or `hypha::Cycle` (section 5.5) when ports are built
  at run time. Missing and stale inputs reach the step as verdicts, and the application decides.
  `examples/scan_cycle/inc/scan_cycle/scan_cycle.h` is a software PLC of this kind.

`latestSample()` is for tests and tools. Code that acts on a topic's newest value at a rate is a
cycle with a `Latest<T>` port; there is no timer-polling pattern. A cycle's timer starts only when
its registration commits, and `node.run()` serves it on the calling thread.
`hypha::require(cycle.health())` and `hypha::require(port.bindingHealth())` read health without
capturing; the node introspection (section 9.1) lists every cycle's topics, period, modes and
limits. Reference: `hypha/cycle_runtime.h` (`Cycle`), `hypha/cycle.h` (`Hz`, `CycleTime`, health
records).

### 5.3 Options

Options are tags in the declaration; the port table of the parameter file is the wiring (`topic`,
`silence_ms`, `stagnation_ms`, `ignored`). There is no options struct and no setter. The parameter
file is loaded before the application is constructed, so a member initializer can compute a port's
options and even its name. `scan_cycle.h` declares an optional advisory input;
`examples/mrs/managers/transform_manager.cpp` uses `latched` and `unsupervised`,
`examples/mrs/managers/mission.cpp` `perStep(64)`.

| Tag | `Latest` | `Arrivals` | `Output` | `advertise` | `subscribe` |
|---|---|---|---|---|---|
| `optionalTopic` | port | port | ✓ | port | port |
| `advisory`, `unsupervised`, `limitMax(d)` | ✓ | ✓ | | | |
| `hostClock` | ✓ | | | | |
| `perStep(n)` | | ✓ | ✓ | | |
| `newestOnly` | | ✓ | | | |
| `latched`, `maxPayloadDynamic(b)` | | | ✓ | ✓ | |
| a `hypha::Qos` value | ✓ | ✓ | ✓ | ✓ | ✓ |
| `diagnostic` | | | ✓ | | |

"port" marks a tag only for a port that names a table: a port on a `hypha::Topic{...}` literal, or
one that shares another port's topic, takes no `optionalTopic`, and a sharing port takes no `Qos`.
Any other combination does not compile. What each tag does: `hypha/options.h`.

The `topic` of a port table: without `optionalTopic`, `topic = ""` fails with `InvalidArgument` and
a missing key or table with `NotAvailable`; with it, both disable the port. An invalid name
(`InvalidTopic`), a number or bool, and a plain entry `name = "/x"` (`InvalidArgument`) fail either
way. Each is a refused configuration: `runNode` exits 78, and `Node::subscribe` and `Node::advertise`
throw `hypha::ConfigError`, so a table refused in `setup()` also exits 78.

Three input tags answer three separate questions; none implies another:

| Tag | Question | `silence_ms` | Counts for `inputsOk()` and node state |
|---|---|---|---|
| `optionalTopic` | May the deployment leave the port unwired? | as without the tag | a disabled port is not judged |
| `advisory` | Is the node healthy without this input? | as without the tag | no |
| `unsupervised` | Has the input no age limit by nature? | refused; never stale | yes, unless also `advisory` |

Every enabled input states its time limit, advisory or not: a `silence_ms` in its table or the tag
`unsupervised` ("Input supervision"); without either, `start()` fails. An input is *untimed* (has no
time limit) when its table sets `silence_ms = 0` (a ROS timeout of 0.0, the deployment's choice) or
its code tags it `unsupervised` (an input that never has one: a configuration topic, a port without
a table). An untimed input is `NeverArrived` before its first sample, `Usable` after, never
`stale()`; `port.untimed()` says so. `advisory` and `unsupervised` combine: an input that may never
be published and never expires (an operator's pause switch). A required untimed input keeps the node
`WAITING` until its first sample. `ignored = true` in the table is the operator's override of what
`advisory` promises: the port starts with its fault overridden. An untimed input has no fault, so the
key is refused on it.

```cpp
// twist_mux: a lock masks the velocities below its priority. One declaration for every
// lock; the table of each says whether it expires (silence_ms = 500) or not (0).
hypha::Latest<std_msgs::Bool> lock{*this, name, hypha::advisory};

bool masks() const  // ROS: hasExpired() || getMessage().data
{
    return this->lock.usable() ? this->lock->data : !this->lock.untimed();
}
```

### 5.4 Cycle node (L3)

A `hypha::CycleNode` is a class: ports are members and `step()` is a method. Use it for a fixed
set of topics, one rate or set of triggers, and state in one object. The cycle has the node's name.
`examples/speed/src/speed_monitor.cpp` and `examples/scan_cycle/inc/scan_cycle/scan_cycle.h` are
cycle nodes. `scan_cycle` is the reference one: its [README](../examples/scan_cycle/README.md)
states each input's contract and has a test at three levels (the logic alone, the step through
`StepHarness`, the started cycle on the bus).

```cpp
#include <hypha/hypha.h>
#include <std_msgs/Float64.h>
class Controller final : public hypha::CycleNode {
  public:
    Controller() : hypha::CycleNode() {}
    hypha::Latest<std_msgs::Float64> speed{*this, "topic_speed"};
    hypha::Arrivals<std_msgs::Float64> setpoints{*this, "topic_setpoint"};
    hypha::Output<std_msgs::Float64> command{*this, "topic_command"};
    void setup(hypha::Node&) override {
        this->setRate(hypha::Hz(100.0));
    }
    void step(const hypha::CycleTime&) override {
        if (!this->speed.usable()) {
            return;
        }
        this->command.send({.data = -this->speed->data});
    }
};
int main(int argc, char** argv) {
    return hypha::runNode<Controller>(argc, argv, "controller_node");
}
```

```toml
[ports.inputs.topic_speed]
topic = "/speed"
silence_ms = 100
[ports.inputs.topic_setpoint]
topic = "/setpoint"
silence_ms = 500
[ports.outputs.topic_command]
topic = "/command"
```

**Phases.** `runNode<App>(argc, argv, nodeName, constructorArguments...)` constructs
`App(constructorArguments...)` and runs, in order: the ABI check (section 2.1), *configuration
loading*, *application construction* (read and validate `hypha::config()` here; no bus yet),
*node creation*, *setup* (`setup(Node&)`: endpoints, rate or triggers), *cycle start*,
*configuration check* (no key left unread), *node report* (section 2), *started*
(`started(Node&)`), *run*. Then the application is destroyed and the `Node` leaves the bus. A
failure logs `node failed in <phase>: <what>`.

**Exit codes** (0, 2, 70, 75, 78; `hypha/application_node.h`; restarts: tools.md, `hypha launch`):

- 78, a refused configuration: a `hypha::ConfigError` (a `ParameterError` among them) thrown from
  the constructor or `setup()`; a bad `node.name` or `[log]` table, refused when the file loads; and
  `failRun(hypha::ConfigError("..."))` from any thread, for a configuration fault only the running
  application can see (a first frame over the configured limit, a pixel format the device lacks),
  so a supervisor does not restart it. The first report decides the code.
- 70: `failRun(reason)`; a bad `HYPHA_USE_SIM_TIME` or `HYPHA_REPLAY_LOCKSTEP`, a node name given in
  code, or a name another process holds (all fail *node creation*).
- 75: `ClockRewound`. A program with its own `main` returns 75 itself to be restarted
  (`replay-flow-control.md` §4.4).

Derive an application that owns worker threads or composes cycles
(`examples/sensor_fusion/src/fusion_main.cpp`) from `ApplicationNode`, and one that steps at a rate
or on its triggers from `CycleNode`.

- **The rate has one source**: `rate_hz` under `[cycle]`, `setRate()` in `setup()`, or triggers
  (`setTrigger(port)` once per trigger port). `rate_hz` with `setRate()` fails the start (`keep
  one`), and so does none (`neither a rate nor a trigger`).
- **Port topics.** An input takes `{cycle, "name"}`, `{cycle, hypha::Topic{"/x"}}`,
  `{cycle, "name", hypha::Topic{"/x"}}` or `{cycle, ["name",] hypha::sharing(other)}`, then its
  options; an `Output` takes a port name only (`hypha/cycle_latest.h`).
- **Start.** `start()` binds the ports in declaration order. A failure names cycle, port and status:
  `hypha: cycle 'controller_node': input 'topic_speed': NotAvailable`.
- **One `CycleNode` per process.** A second step is a composed `hypha::Cycle` (5.5); a second rate
  inside the step is a `CycleTimer` ("A second rate in a step").
- **Wake-up.** On host time a rate step starts when the run wakes at its deadline: on Linux within
  the kernel's wake-up latency, on macOS late by up to half the wait and at most about 10 ms
  (execution-model.md §11). `CycleTime::now` is the wall time at the step's start, not the deadline.
- **Queues.** A `Latest` subscriber queues the policy's `maxDepth`, an `Arrivals` its `perStep`
  (section 7). The cycle history is sized below the ABI (`buffer_size + 1` for `Latest`,
  `buffer_size + perStep` for `Arrivals`) and counts samples, not bytes.

#### Sharing a topic

`hypha::sharing(first)` makes a port a second view of another port's samples, with its own limit
keys and cursor, in any cycle (`hypha/cycle_input.h`). In
`examples/mrs/host/include/mrs_host/plugin_host.h` a second subscription shares the first.

#### Queued work, budget and overflow

An `Arrivals` port examines at most `perStep()` samples per step (default 8) and queues that many,
so `perStep()` is at most the `maxDepth` of the port's policy: 32 for `default`, at most 1024 with
a policy of your own (section 7). A larger one throws `std::invalid_argument` naming the policy at
construction, and `runNode` exits 70: a `perStep` fixed in code is a programming error. The
framework cannot tell a number came from the parameter file; to refuse one as a configuration fault
(78), read it with its range:

```cpp
Arrivals<Imu> imu{
    *this, "imu",
    perStep(config("ports.inputs.imu").required<uint32_t>("queue_size", constraints::range(1, 32)))};
```

A sample the age rule refuses still uses a slot. Pending samples beyond a share are the oldest; they
are dropped and counted (execution-model.md §6.5). A `CycleNode` caps all queued ports together
with `arrival_budget` (a positive integer, read at commit) in `[cycle]`, beside `rate_hz`:

- Each queued port asks for its pending samples up to its `perStep()`. Asks above the budget share
  it equally among the ports with work; a port that needs less frees the rest. A remainder smaller
  than the port count goes one sample each, starting from a port that rotates every step.
- `Latest` ports, idle ports, triggers and a `Transforms` port take nothing from the budget. Without
  a budget each port uses only its `perStep()`.
- `rate_hz` and `arrival_budget` are the only keys of `[cycle]`. Another key or a value out of
  range fails the commit, and the log names the key and value. Put root-level keys above the first
  table header. A composed `Cycle` reads none.
- Simulated time steps one deadline at a time and holds a deadline for its upstream nodes
  (execution-model.md §11.1). `hostClock`: `hypha/options.h`.

#### Triggered cycles

`setTrigger(port)` before `start()` adds an `Arrivals` port to the cycle's triggers; the cycle then
runs one step per trigger sample instead of at a rate. Call it once per trigger port; a repeat adds
nothing and order does not matter. The rules are in execution-model.md §6.6;
`examples/mrs/managers/estimation_manager.cpp` and `control_manager.cpp` are triggered cycles.

- **Order.** Trigger samples step in send-stamp order, ties in port construction order. In a step
  the port that fired holds its one sample and every other trigger holds none: the port whose
  `samples()` is not empty fired.
- **Time.** `CycleTime::now`, and so every output's send stamp, is the trigger sample's send time T
  on simulated time (a fallback's own time), and the wall time at the step's start on host time. On
  host time an output therefore does not carry the trigger's stamp: copy the sample's
  `header.stamp`, or read `port.sentAt(0)`, its host-monotonic send time (not a wall time)
  (execution-model.md §6.6 item 2).
- **Limits and fallback.** Each trigger has its own `silence_ms`; `usable()` and `stale()` of every
  trigger judge its newest sample against it in every step. After the shortest `silence_ms` with no
  step, a fallback step runs with every trigger empty, and each trigger past its own limit is
  `stale()`. An untimed trigger (`silence_ms = 0` or `unsupervised`) is never `stale()` and sets no
  fallback; a cycle whose triggers are all untimed runs no fallback and no step before its first
  trigger sample, as a ROS callback with no timeout.
- **Other `Arrivals` ports** hold, in a trigger step, the samples sent up to the trigger sample,
  including those one upstream step sent with it; a later one is in a later step. On simulated time
  that is every sample stamped at or before T, on every port.
- **Refused at `start()`**: a rate with a trigger ("a rate and a trigger exclude each other"), a
  `Latest` trigger ("the trigger must be an Arrivals port"), and two triggers on one topic through
  `sharing()`.

**Newest only.** ROS's `queue_size` 1: `Arrivals<Twist> cmd{*this, "cmd", newestOnly}` steps on the
newest pending sample and passes the older ones, as twist_mux's subscribers do.

- On host time a `newestOnly` trigger steps once per turn on the newest of its pending samples, so a
  slow step on a fast topic leaves no backlog; another trigger's sample between them still steps.
- On simulated time a step at T sees samples stamped up to T, and a later stamp is its own step, so
  only a second sample stamped T passes the first. Which sample steps follows from the recording,
  not from when the node got its turn (execution-model.md §6.6 item 7).
- A non-trigger `Arrivals` port with the tag examines one sample per step, the newest, and asks one
  from the `arrival_budget`.
- A passed sample is counted, never silent: `work_limit_drops` of the step's report,
  `bindingHealth().work_limit_drops_total`, and `dropped` of a required input's `~/state` row. The
  queue keeps its `perStep()` depth, so a burst between two turns is not a transport loss, and the
  publishers and topic policy are untouched.
- A plain `subscribe` callback has no counterpart: port a `queue_size` 1 callback as a triggered
  cycle. C ABI: `HYPHA_CYCLE_INPUT_MODE_NEWEST`.

To test a triggered step in a `StepHarness`, `deliver()` the sample of the port that fires, and
`deliver(port, {}, sentAt)` each other trigger with the send time of its newest sample (or keep a
delivery from an earlier step), so its `usable()` is judged as in the cycle.

**Clock driver.** A rate cycle that drives `/clock` for a simulator: `setClockDriver(options)` with
`setRate()`, before `start()` (`hypha/cycle_runtime.h`, `hypha/cycle_clock_driver.h`;
`examples/mrs/simulator/mrs_simulator.cpp`). It shares its acknowledgement rules with `hypha play`
(`replay-flow-control.md` §4.5). The start value goes out with no step; the first step runs at start
+ period, once every name in `participants` acknowledged the start. So a ported simulator publishes
its initial state in that first step, not at the start value as a ROS 1 simulator does with its first
`/clock`. The start value is 0 to, not including, 2^32 s (the range of `/clock` seconds); `start()`
refuses another, naming the range.

#### A second rate in a step

A `hypha::CycleTimer` (`hypha/cycle_timer.h`) is the `ros::Timer` of a step: a member the step polls
with its own time, for work at a second rate over the step's state (a 10 Hz status beside a 100 Hz
loop, a gains update, a loop upstream ran from its own timer). It is a value on `CycleTime::now`, so
it ticks the same on host time, simulated time, in a replay and in a `StepHarness`, and two replays
give the same ticks. It adds nothing to the C ABI and allocates nothing.

```cpp
hypha::CycleTimer status{hypha::Hz(10.0)};                  // running; anchored at its first poll
hypha::CycleTimer gains{hypha::Hz(10.0)};                   // gains.stop() in the constructor
hypha::CycleTimer loop{hypha::Duration::fromMillis(10)};    // upstream's 100 Hz timer body

void step(const hypha::CycleTime& time) override {
    if (const auto tick = this->status.poll(time)) {        // first, before the ports
        this->publishStatus(tick.missed());                 // once, missed periods counted
    }
    for (auto n = this->loop.poll(time).periods; n > 0; --n) {
        this->onLoop();                                     // or once per period
    }
    // ... then read the ports; activate() calls gains.start(time), deactivate() gains.stop()
}
```

- `poll(time)` returns a `Tick`: `periods`, the grid instants in (previous poll, now]; `expected`,
  the newest of them (ROS `current_expected`); `elapsed`, the time since the previous tick's step
  (ROS `current_real - last_real`), empty for the first tick after anchoring. A `Tick` is true when
  `periods` is not 0.
- The first poll anchors the grid at the step's time and gives no tick. An early step gets none, a
  late one more than one; the grid never drifts. Periods beyond the first are missed:
  `tick.missed()`, and `missedTotal()` since construction. Running the body once is the hypha timer
  rule (execution-model.md §11); once per period is what a ROS timer did after a slow callback, and
  what a port that must match upstream step for step writes. The count has no cap; the body chooses.
- `stop()` ends the ticks, and a stopped timer misses nothing. `start(time)` starts a stopped timer
  one period after `time`; `start()` anchors it at its next poll. On a running timer either changes
  nothing, as ROS `start()`; `stop()` then `start(time)` re-anchors it.
- Poll it first, in every step, as ROS ran a due timer before the next callback; a step that does
  not poll leaves its periods to the next poll's count. Time that goes back (a host wall-clock step)
  re-anchors the grid.
- It wakes nothing: a tick runs in the first step at or after its instant. In a triggered cycle that
  is a trigger or fallback step, so the shortest `silence_ms` bounds how late it runs. A cycle with
  no fallback (untimed triggers, #238) runs no step while its triggers are silent, and that stretch's
  periods arrive as the next step's count. Work that must run on its own deadline is a rate cycle.

#### Loss counters

Each loss has its own counter (execution-model.md §6.5); do not add them up. iceoryx2 counts no
queue evictions: a transport drop shows only as a gap in `overrun_total`.

| Counter | Reads | Counts |
|---|---|---|
| `lost()` | `Arrivals` | Samples the history overwrote before this port read them. |
| `bindingHealth().overrun_total` | every input | History losses and, for a queued input, transport sequence gaps. A `Latest` input counts neither on host time; on a replay it counts transport gaps and the evictions of execution-model.md §11.1. |
| `bindingHealth().age_rejections_total` | every input | Samples the age rule refused: received, not lost. |
| `bindingHealth().work_limit_drops_total`, `.capture.work_limit_drops` | queued inputs | Samples over the step's share, dropped on purpose, and samples a `newestOnly` port passed for a newer one; `capture.` counts the latest step only. |
| `bindingHealth().cleared_total` | every input | Pending samples a step's `clear()` dropped (below). A rate step leaves none pending. |

#### Topic states of an input port

Enabled or disabled is configuration (the port's `topic`, section 5.3); counting for health is code
(`advisory`). `examples/scan_cycle/inc/scan_cycle/scan_cycle.h` declares its setpoint
`optionalTopic, advisory` and falls back when the port is switched off.

- A disabled port has no subscriber and never a value. The introspection lists it with an empty
  topic, and the node state does not judge it.
- A disabled port reports `NeverArrived`, like an enabled port with no sample yet. Tell them apart
  with `enabled()`, after `start()` or in a `StepHarness`.
- An output follows the same rules; a disabled output has no publisher.
- A failed `start()` reads `hypha: cycle 'control': input 'speed': NotAvailable`, or, with a reason
  from the library, `hypha: cycle '<name>': <StatusName>: <reason>`.

#### Input values

`examples/speed/src/speed_monitor.cpp` reads `usable()`, `verdict()` and `*port` of a `Latest`
input; the headers describe every accessor. Two counters on every input, both wrapping:

- `received()`: samples the source accepted, including those the age rule refused or the history
  overwrote.
- `deliveredCount()`: usable samples that became the value (one per new `Latest` value, each usable
  `Arrivals` sample). Use it as a message counter.

An output's `send()` serializes into one of its `perStep()` loans at once; the messages go out after
the step returns, in `send()` order (execution-model.md §6.1, failures §6.3). The log (module
`hypha`) shows a step exception's text once with the cycle name (no text for a non-`std::exception`).

- A step stamp that `RosTime` cannot hold fails the step with `InvalidState`; the step is not called,
  and `runNode` returns 70.
- A port's destructor releases its cycle. While the `WaitSet` runs, the destroy is refused: the cycle
  fails at its next tick with `InvalidState` without touching the freed ports, and the node keeps it
  until the `Node` is destroyed. A cycle that outlives its `Node` does nothing when destroyed.

Reference: `<hypha/cycle_latest.h>`, `<hypha/cycle_arrivals.h>`, `<hypha/cycle_output.h>`,
`<hypha/cycle_input.h>`, `<hypha/cycle.h>`.

#### Test a step without a bus

`hypha::testing::StepHarness` (`<hypha/testing.h>`, not in `hypha/hypha.h`) runs the step of a cycle
that was not started: no node, no shared memory, and a parameter file only for optional ports and
`deliver()`. `crates/hypha-cxx/test/test_cycle_doc_examples.cpp` tests the controller above with it;
the header gives the setters and rules, and `testing.h` lists what a harness step shares with a
started cycle and what it leaves out. `send()` outside a step throws
`std::logic_error("hypha: output '<port>': send() outside a step")`, so a `setup()` that sends fails
start-up: send an initial message from the first step.

**Which ports are enabled.** The harness predicts what a start would enable, from the parameter file
a `Node` or `ConfigContext` holds when the harness is built.

- An `optionalTopic` port is enabled when its table (`[ports.inputs.<name>]` or
  `[ports.outputs.<name>]`) sets a non-empty `topic`. A missing table or `topic`, `topic = ""`, or no
  file leaves it disabled, so the step takes the unwired branch. The constructor throws `ConfigError`
  for an optional port whose `topic` a start would refuse, or whose entry is a plain string
  (`mag = "/x"`), and `ParameterError` for a `topic` that is not text.
- A port that requires its topic, or sits on a `hypha::Topic{...}` literal, is enabled without a
  check; a start refuses a bad one, the harness does not.
- To test an optional port's branch, wire it in the test's file: `deliver()` to the input gives the
  step its three verdicts (usable, stale, never arrived), and `sent()` keeps what the step sends. An
  enabled output reports a subscriber (`hasSubscribers()` true).
- On a disabled input, `deliver()`, `set()`, `setStale()` and `setStagnant()` throw
  `std::logic_error` naming the table that lacks a `topic`; `setNeverArrived()` is allowed, since it is
  true of a disabled port. A disabled output counts sends and keeps none, so its `sent()` is empty.
- The harness opens no endpoint and checks no table keys but `topic`, `silence_ms` and a service
  port's keys.

**Feeding inputs.**

- `harness.deliver(port, value, sentAt)` gives a `Latest` port a value sent at `sentAt`. Each later
  `step(time)` judges it as a capture does: usable while `time.now - sentAt <= silence_ms` (read at
  `deliver()`, so a `Node` or `ConfigContext` must hold the file), stale after. A port without
  `silence_ms`, without a table, or `unsupervised` never goes stale. `fresh()` is true in the first
  step that finds the value usable; `lastKnown()` keeps a value that was usable once. A value stale in
  its first judged step is never held. A `sentAt` after the step time is stale with age 0 until then
  (a real cycle on simulated time does not admit such a sample at all).
- `set()`, `setStale()` and `setNeverArrived()` state a verdict directly; a later `set()` or
  `deliver()` replaces it. Counters advance as on the bus: the next step counts each given value in
  `received()` and each usable one in `deliveredCount()`, so two values before one step are two
  received, one delivered. `setNeverArrived()` zeroes both.
- `harness.deliver(port, samples, newestSentAt)` does the same for an `Arrivals` port: the samples go
  to the next step only, and each later step judges the port by `newestSentAt`, so a stopped publisher
  gives `Stale` steps. Samples whose newest is already stale in the taking step are not delivered.
- `harness.deliver(port, transforms)` gives a `Transforms` port a
  `std::vector<geometry_msgs::TransformStamped>` as from `/tf`; `deliverStatic` as from `/tf_static`.
  Lookups answer as a started cycle's TF input does, from the same library code: interpolation between
  stamps, static transforms at every time, the port's `cache` per frame, `UnknownFrame` for an
  undelivered frame (`NotConnected`, `BeforeBuffer`, `AfterBuffer` as in a cycle), and `Refused` for a
  frame whose transform the input refused (not finite, not a unit quaternion, a second parent). The
  transforms stay for the harness's life.

```cpp
harness.deliver(node.tf, {odomInMap});          // geometry_msgs::TransformStamped values
harness.deliverStatic(node.tf, {lidarInBase});
harness.step(node, time);                       // node.tf.lookup("map", "lidar", at) finds it
```

**Services.** The test is the service peer (`hypha/testing/step_harness_services.h`); there is no
fake server or client.

- `harness.request(port, request, {.client, .sentAt})` delivers a request to a `Requests` port, as
  `rosservice call` would, and returns a probe: `queued()` (no step read it), `deferred()`,
  `replied()` with `response()`, `refused()` or `failed()` with `reason()`, `answeredAt()`, and
  `describe()` for a test message.
- `harness.calls(port)` lists the requests the last step sent on a `Calls` port.
  `harness.answer(port, response)` and `harness.outcome(port, hypha::CallOutcome::Timeout, "after 1
  s")` end its oldest call in flight (`Refused`, `NoServer`, `NotSent`, `ServerLost` or `Timeout`),
  read by the next step, or with a time `at` by the first step after it. With no call in flight they
  throw `std::logic_error`.
- The library runs a harness step's services as a started cycle's on simulated time, so section 8.5
  holds: a step at T reads requests sent before T (by default just before the next step), in order, at
  most `per_step`; answers and calls leave after the step; a request read and left alone is refused; a
  step that throws answers what it read as failed, ends its calls `NotSent`, and passes unread outcomes
  to the next step, where a successful step drops and counts them; a call open past its timeout ends
  `Timeout`.
- `per_step`, `outstanding` and `timeout_ms` are read from the parameter file and refused as a start
  refuses them; a port without a table keeps the defaults.

```cpp
UavManager                  node;  // section 8.6
hypha::testing::StepHarness harness{node};
auto takeoff = harness.request(node.takeoff, {});
harness.step(node, at(0));
ASSERT_EQ(harness.calls(node.switchController).size(), 1U);
harness.answer(node.switchController, {.success = true});
harness.step(node, at(10));
harness.outcome(node.switchTracker, hypha::CallOutcome::Timeout, "after 1 s");
harness.step(node, at(20));                     // disarms: toggle_output first
harness.answer(node.toggleOutput, {.success = true});
harness.step(node, at(30));                     // then arming
harness.answer(node.arming, {.success = true});
harness.step(node, at(40));
ASSERT_TRUE(takeoff.replied()) << takeoff.describe();
EXPECT_EQ(takeoff.response().message, "could not activate the tracker (timeout after 1 s)");
```

**Clock.** The harness moves no clock. In a started cycle `hypha::now()` returns `CycleTime::now`
during the step; in a harness step it returns the process clock, so step code reads the `CycleTime`
it is given. A test of code that still calls `hypha::now()` sets simulated time itself:
`hypha::enableSimTime(time.now)` once, `hypha::setSimTime(time.now)` before each `step()`,
`hypha::disableSimTime()` at the end. `steadyNow()` is real time and is never set.

**Replaying a recording.** `hypha::testing::McapReplay` (`<hypha/testing/mcap_replay.h>`) replays an
MCAP file of ROS 1 messages into a harness, so a golden-sample test is one gtest.

- `feed(port)` takes the topic each `Latest` or `Arrivals` input resolves (a table topic needs a
  `Node` holding the parameter file). `feedIfPresent(port)` returns `false` and leaves the port unfed
  when the file lacks the topic, for an input only some recordings hold.
- `next(harness)` delivers one step's samples and returns its `CycleTime`, while `stepDue()`.
  `read<M>(topic)` returns every message of a topic, decoded, with log and publish time, for golden
  outputs.
- Steps lie on the period's grid, as in a replay cycle (execution-model.md §11.1): the first is the
  first multiple after the earliest fed sample, and each step gets the samples whose log time (the
  stamp `hypha play` sends) is strictly before its deadline. Both port kinds get samples through
  `deliver()`, so each step judges a port by its newest sample.
- `endAt(time)` adds the silent tail: steps continue through the last deadline at or before `time`
  (for a golden topic's end, `read<M>(topic).back().logTime`), so a test sees ports go stale and
  outputs follow.
- Link `hypha::testing_mcap` (option `HYPHA_BUILD_TESTING_MCAP`, on by default) beside `hypha::sdk`. A
  refusal throws naming the fault, file and topic. `crates/hypha-cxx/test/test_mcap_replay.cpp` tests
  it against a started cycle on the bus.

```cpp
hypha::testing::StepHarness harness{detector};
hypha::testing::McapReplay  replay{"fixture.mcap", hypha::Hz(20.0)};
replay.feed(detector.position);
while (replay.stepDue()) {
    const hypha::CycleTime time = replay.next(harness);
    harness.step(detector, time);
}
```

#### Input supervision

Give each input that counts for health a silence limit in its table. For an input whose data comes
through a relay (a device driver, the ROS bridge, a gateway), declare its life sign in code:
`arbiter.setAlive(&VehMode::Life_Cnt_Arbiter)` before `start()`. An input from a hypha cycle needs
none, and an unchanging payload is not stagnant
([communication-integrity.md](communication-integrity.md)).
`examples/sensor_fusion/config/fusion.toml` sets `silence_ms` and `ignored` per sensor; the mrs
managers (`examples/mrs/managers/`) tag inputs with no age limit `unsupervised`.

```toml
[ports.inputs.arbiter]
topic = "/vehicle/mode"
silence_ms = 250
stagnation_ms = 500      # only with setAlive()
ignored = false          # optional; default false
```

The parameter file cannot switch supervision off, and code sets no limit value:

- `silence_ms` is required on an enabled input, `advisory` or not, unless `unsupervised`, and refused
  on an `unsupervised` one. `silence_ms = 0` is no limit. `stagnation_ms` is required with
  `setAlive()` and refused without it.
- A missing required key fails `start()` with `NotAvailable`; a `CycleNode` under `runNode` exits 78
  (a composed cycle exits as the application decides):
  `hypha: cycle '<cycle>': input '<port>': missing key 'silence_ms': an input needs a limit, 0 for none, or the tag hypha::unsupervised: NotAvailable`.
- A limit is 0 (none) or 1 ms to 3600 s; `limitMax(d)` lowers the top for the port. A value outside
  fails `start()`; nothing is clamped. The limit is inclusive: a value exactly `silence_ms` old is
  usable.
- An unknown key fails `start()`, naming port and key. Known keys: `topic`, `ignored`, `silence_ms`,
  `stagnation_ms` with `setAlive()`, and any key the application read before `start()` (section 9,
  port tables).
- A sharing port has no `topic` key. An enabled port without a table name reads no limit and needs
  `unsupervised`, else `start()` fails with `an input without a table has no time limit and needs the
  tag hypha::unsupervised`.
- A port with no silence limit and no `setAlive()` is never stale. A disabled port needs no limits.
  `ignored = true` starts the port with its fault overridden, and fails `start()` on a port with no
  limit.
- The alive value of a `Latest` port is read from its value, of an `Arrivals` port from the step's
  newest sample; a step with no sample keeps the last value.

There is no state enum: `!enabled()`, `received() > 0`, `usable()`, `stale()`, `stagnant()`,
`ignored()` and the cycle's `inputsOk()` answer the questions. `bindingHealth()` returns the counts
with the supervision report; its `.stale` and `.stagnant` show the faults even while `ignored`
overrides them, as the `~/state` row does, while `stale()` and `stagnant()` hide an ignored fault.
Each change is logged once (execution-model.md §6.4 lists the lines). Reference:
`<hypha/cycle_input.h>` (`stale()`, `stagnant()`, `setAlive()`, `setIgnored()`),
`<hypha/cycle_runtime.h>` (`inputsOk()`).

#### A reset: `clear()`

A node that resets (robot_localization's `reset` and `set_pose`, an EKF restart) forgets what its
inputs and TF input hold with `clear()`, in the step:

```cpp
void RosEkf::reset()                        // called from step()
{
    for (OdomInput& input : this->odom_inputs) {
        input.port.clear();                 // hypha::Arrivals<nav_msgs::Odometry>
    }
    this->transforms.clear();               // tf2's tfBuffer_.clear(); static ones stay
    this->filter_.reset();                  // the node's own state, as upstream
}
```

The clear applies when the step returns; the rest of the step still reads its capture. Later steps
see only later samples. Until one arrives the input has no value (`has()` false) and is
`NeverArrived`, and it turns `Stale` when the newest pre-clear sample passes `silence_ms`: a clear
restarts no timer and hides no silent source. A trigger port cannot be cleared (`std::logic_error`).
Dropped pending samples count in `bindingHealth().cleared_total`. A `Transforms` port forgets its
dynamic transforms and keeps the static ones; lookups miss until a new transform arrives. On a
replay a clear at step T forgets what T's cut admitted, the same in every run and `--chunk`. In a
`StepHarness`, `harness.cleared(port)` says whether the last step cleared the port. Rules:
[execution-model.md](execution-model.md) §6.8.

### 5.5 Cycle composition (L2)

Use `hypha::Cycle` when ports are built at run time, when one state object serves two rates, or when
the ports are not members of a class. The step is a named functor.
`examples/sensor_fusion/src/fusion_main.cpp` builds `sensor_count` inputs this way, and
`examples/sim_twin/src/sim_twin.cpp` runs three composed cycles on `/clock`.

```cpp
struct Relay {
    std::deque<hypha::Latest<std_msgs::Float64>>* inputs;
    hypha::Output<std_msgs::Float64>*             sum;

    void operator()(const hypha::CycleTime&) const {
        double total = 0.0;
        for (const auto& input : *this->inputs) {
            if (input.usable()) {
                total += input->data;
            }
        }
        this->sum->send({.data = total});
    }
};

hypha::Cycle cycle("relay");
std::deque<hypha::Latest<std_msgs::Float64>> inputs;   // a deque: elements never move
for (const std::string& parameter : topicParameters) {
    inputs.emplace_back(cycle, parameter);
}
hypha::Output<std_msgs::Float64> sum{cycle, "topic_sum"};
cycle.setRate(hypha::Hz(50.0));
hypha::require(cycle.start(node, Relay{&inputs, &sum}));
```

A composed cycle reads no `[cycle]` table. Declare it before its ports, so each port is destroyed
first and releases the cycle. The rules of §5.4 apply to its ports, triggers and supervision.
Reference: `<hypha/cycle_runtime.h>`.

## 6. Raw and type-erased API

A recorder, player, bridge or benchmark cannot name its types at compile time, so it sends a topic
identity and bytes. `examples/large_payload/src/large_source.cpp` publishes loans of a
`rawPublisher`, `large_sink.cpp` reads them with `subscribeRawOfType`, and
`examples/bench/src/bench.cpp` uses `subscribeRaw`.

- A `RawType` needs a data type and an md5 of 32 lower-case hex digits. The md5 names the stream, so
  `subscribeRawOfType(topic, type, ...)` reads `type.md5` alone, and `subscribeRaw` (no type) reads
  every stream of each matching topic that exists at creation. On a literal topic it wakes on any of
  them; a pattern is polled every 1 ms (execution-model.md §8).
- A raw endpoint without a `hypha::Qos` adopts the stream's service (section 7).
- A raw callback's bytes are valid only during the callback; copy them to keep them.

Reference: `<hypha/raw_api.h>`, `<hypha/raw.h>`, `LoanedBuffer` in
`<hypha/backend/raw_publisher.h>`.

## 7. Service limits and endpoint queues

A topic's service limits (publishers, subscribers, deepest queue) size its shared memory, so they
belong to the topic, per stream of each type (section 5): two types, or two versions of one type, on
one name have their own limits. A *policy* names them. Keep an application's policies in one header
that both ends of every topic include, as `examples/large_payload/inc/large_frame_qos.h` does:

```cpp
// my_stack/qos_policies.h: included by every node of the stack
#include <hypha/qos.h>
#include <my_msgs/Image.h>

namespace my::policies {
inline constexpr hypha::Qos camera{
    .name = "camera", .publishers = 1, .subscribers = 4, .maxDepth = 2,
    .maxPayloadDynamic = 8'000'000};
}

template <>
struct hypha::QosOf<my_msgs::Image>
{
    static constexpr hypha::Qos value = my::policies::camera;
};
```

- **Every typed endpoint has a policy**: the port's `hypha::Qos` option, else `QosOf<M>::value`.
  hypha maps four types itself, and an application cannot remap them: `tf2_msgs/TFMessage` to `tfQos`,
  `diagnostic_msgs/DiagnosticArray` to `diagnosticsQos`, and the node report types `NodeState`
  (`~/state`) and `NodeIntrospection` (`~/introspection`) to `reportQos` (2 publishers, 2 subscribers,
  depth 1, 1 KiB first slot; only tools read them, from the observer slots). For large messages
  `hypha::imageQos` and `hypha::cloudQos` are port options, mapped to no type (section 7.1).
- **Node slots** are `max_publishers` plus the admitted subscribers, plus 2 kept for a node's refresh
  of the topic's other versions and the record check when an endpoint drops. Neither takes a slot an
  endpoint needs, and the 2 admit no extra endpoint
  ([interface-specification.md](interface-specification.md#streams)).
- **Mismatch.** A later endpoint of the same definition (MD5) with another policy is refused with
  `QosMismatch`, and the log names both policies: `advertise` and `subscribe` throw, a cycle fails
  `start()`. Another MD5 on the same name is its own stream and is not refused; two type names of one
  definition share a stream, so their `QosOf<M>` must agree.
- **Start order.** A raw endpoint, player or bridge that opens a topic first creates it with
  `default`, and a typed endpoint with another policy is then refused. Start the application nodes
  first, or map the type to `default`.
- **Replay.** On simulated time a sample a queue drops fails the run (`SequenceGap`, "replay not
  faithful"). A node drains its inputs on every `/clock`, so under `hypha play` a queue holds one
  chunk, which the player splits to fit; an `Arrivals` port needs a `perStep()` of what the topic
  sends in one period of its cycle (`replay-flow-control.md` §6.7).
- **Shared topics.** Ports that share a topic read one subscriber with the deepest `perStep` among
  them; a port with a smaller `perStep` drops a step's older samples as work-limit drops.
- **Latching.** A latched sample reaches only the subscriber that joined, so a join adds no
  `integrity.sequence_gap` rejection at a connected subscriber.
- **Subscriber counters.** `sub.missingSequences()` counts frames lost before they were read (a queue
  overflow adds to it; a loss before a publisher's first accepted frame counts only when the publisher
  came after the subscriber, `communication-integrity.md` §3). `sub.metrics()` returns a
  `hypha::SubscriberMetrics`: `sequenceGap` counts frames that arrived and were refused as not newer
  (a lost frame raises `missingSequences()`, not `sequenceGap`); `invalidFrame`, `corruptPayload`,
  `schemaMismatch` and `invalidTimestamp` count the other refusals; `loanBudget`, `loanHeld` and
  `loanRefused` describe held samples. Both read from any thread while the node runs and give zeros
  for a disabled subscriber. A `RawSubscriber` has the same two; a cycle port has neither.

### 7.1 Sizing the pool of a large message

An image or point cloud is megabytes, and a publisher's pool holds many of them, so the policy and
payload bound decide the topic's shared memory. Three steps:

1. **The message size** in serialized bytes: `hypha::serializedSize(head, n)` (`hypha/message_size.h`)
   is the size of a message whose single array holds `n` bytes, from a `head` that names the other
   fields (`frame_id`, `encoding`, a cloud's `fields`); `head`'s array is not read, so leave it empty.
   An `Image` is `37 + frame_id + encoding` bytes around its pixels (48 for `"camera"` and `"mono8"`);
   a `PointCloud2` is 100 to 200 bytes of field list around its points. For a cloud of varying size,
   `n` is the largest point count you accept times `point_step`; a larger cloud is `TooLarge`.
2. **The pool** of one publisher:

   ```text
   slots = (subscribers + 3) x (maxDepth + 2) + 1 + 2
   bytes = slots x (message + 104, rounded up to a multiple of 8)
   ```

   `subscribers` and `maxDepth` are the policy's. The 3 are observer slots for tools and bridges, the
   2 of `maxDepth + 2` the samples a subscriber holds at once (a callback, or a kept sample), the 1 the
   latch, the last 2 the publisher's loans. `slots` is `qos::pool_samples` in Rust; the `default`
   policy (8 publishers, 16 subscribers, depth 32) gives 649. A slot is resident once written, and the
   pool is the most a publisher takes, reached when every subscriber's queue is full; with
   `publishers > 1` each running publisher takes that much.
3. **The policy** is as small as the application: its subscribers are the readers that exist, its
   `maxDepth` the frames a reader may queue. `imageQos` (1 publisher, 2 subscribers, depth 2: 23 slots)
   and `cloudQos` (1, 2, depth 3: 28 slots) are such policies. Both ends name the policy as a port
   option, and each names its own topic, so two cameras need no header of their own:

   ```cpp
   hypha::Output<sensor_msgs::Image> image{
       *this, "image", hypha::imageQos, hypha::maxPayloadDynamic(bound)};
   hypha::Latest<sensor_msgs::Image> frame{*this, "image", hypha::imageQos};
   ```

   A reader that names no policy gets `QosMismatch` at start, with both policies in the log. A third
   subscriber is refused too: declare your own policy in the header above. No type is mapped to these
   policies: a mapping holds for the whole application, so two lidars could not differ, and `hypha
   play` or a bridge would create the topic with `default` and refuse the typed reader after it.

`hypha topic pool --payload <bytes> --publishers 1 --subscribers 2 --depth 2` prints a pool's cost
(an omitted limit is the `default` policy's). Every publisher whose pool is a megabyte or more logs it
at start: `topic '/image' publisher pool: 23 samples of 5039000 bytes, 115897000 bytes of shared
memory when every subscriber's queue is full; policy ..., 2 loans`.

| Topic | Message | Policy and slots | One pool |
|---|---|---|---|
| 5 MP mono8 camera, 2592 x 1944 | `serializedSize(head, 5'038'848)` = 5,038,896 | `imageQos`, 23 slots of 5,039,000 | 115,897,000 bytes (110.5 MiB) |
| 5 MP rgb8 camera, 3 bytes a pixel | `serializedSize(head, 15'116'544)` = 15,116,591 | `imageQos`, 23 slots of 15,116,696 | 347,684,008 bytes (331.6 MiB) |
| 64,000-point lidar, 26 bytes a point | `serializedSize(head, 64'000 * 26)` = 1,664,156 | `cloudQos`, 28 slots of 1,664,264 | 46,599,392 bytes (44.4 MiB) |

The same lidar on `default` with a bound of 1,800,000 bytes is 649 slots of 1,800,104: 1,168,267,496
bytes (1.09 GiB). A depth of 1 (a reader that wants only the newest frame) takes the camera pool to 18
slots, 90.7 MB for mono8. A publisher with no bound is growable: it reserves its pool at its first
slot size (4096 bytes, or the policy's `initialPayload`), and its later segments are never checked.
Name the bound.

**/dev/shm.** A domain's pools are files in `/dev/shm`, and a write past its free space stops the
process with SIGBUS. The sum of the pools of its topics must fit, with room for the rest of the system;
in a container, `shm_size` is that sum and more. At start on Linux, a publisher whose pool exceeds the
free space logs `may need <n> bytes of shared memory` (and refuses in a bounded deployment), and one
that takes more than half of `/dev/shm` warns with the share. A growable publisher is not checked.

A deeper queue, such as an IMU at `maxDepth = 200`:
[ros1-migration-guide.md](ros1-migration-guide.md#105-queue-depth). Pool size and start order:
[engineering-traps.md](engineering-traps.md); overflow and frame checks:
[interface-specification.md](interface-specification.md#service-limits-and-wildcards). Reference:
`<hypha/qos.h>`, and `<hypha/status.h>` for `Status`, `require()` and `ConfigError`.

## 8. Services

A service is a request and its answer between nodes, typed by a ROS 1 `.srv` (design:
[native-services.md](design/native-services.md)). A name has at most one server, across every version
of its type, and a call ends in exactly one outcome. Sections 8.1 to 8.3 cover plain nodes and
`ApplicationNode`s; section 8.5 the cycle ports `Requests<Srv>` and `Calls<Srv>`.

**Shipped services.** The SDK ships `std_srvs` (`Empty`, `Trigger`, `SetBool`) and the ROS 1 Noetic
services of the packages whose messages it ships, each as `<package>/<Name>.h` with `Request`,
`Response`, `TYPE_NAME` and `MD5`, and the `.srv` unchanged, so the MD5 is ROS's: `nav_msgs/GetMap`
(`6cdd0a18...`), `GetPlan`, `LoadMap`, `SetMap`; `sensor_msgs/SetCameraInfo` (`bef1df59...`);
`diagnostic_msgs/AddDiagnostics`, `SelfTest`; `tf2_msgs/FrameGraph`. A ROS client through the bridge, a
recorded call and code written against `nav_msgs::GetMap` meet them as in ROS. The other shipped
packages (`std_msgs`, `geometry_msgs`, `rosgraph_msgs`) have no services. `hypha_generate_messages()`
with `SRV_DIR` generates an application's own, and can add to an SDK package (section 1.1). A map does
not fit the default 4 KiB service bound (section 8.4): the server and its clients include one header
that specializes `hypha::ServicePolicyOf<nav_msgs::GetMap>` with bounds for their map, 64 MiB at most.

### 8.1 A server

```cpp
#include <std_srvs/SetBool.h>

auto server = node.advertiseService<std_srvs::SetBool>(
    "set_enabled", [this](hypha::Request<std_srvs::SetBool>& request) {
        if (!this->ready_) {
            request.refuse("not initialised");   // the client reads Refused
            return;
        }
        request.reply({.success = true, .message = request->data ? "on" : "off"});
    });
```

```toml
[ports.servers.set_enabled]
service = "~/set_enabled"          # /<node>/set_enabled
```

- **The handler** runs on the run thread, between turns, like a subscription callback. It does one of
  three things: `reply(response)`, `refuse()` or `refuse("why")`, or `defer()`. A request left without
  any of them is refused when the handler returns (`the server's handler did not answer the request`),
  counted and logged once. A handler that throws answers its request as failed (the client reads
  `ServerLost`) and fails the run, as any throwing callback does.
- **Names.** `hypha::Service{"/abs/name"}` instead of a port fixes the name in code. A port without a
  table is refused with `ConfigError` (exit 78), and the message prints the stanza to add. A second
  server of a name a live node serves is refused (`ResourceLimit`), naming the holder and its type.
- Destroying the `ServiceServer` ends every request it holds: their clients read `ServerLost`.

**Deferred answers.** `request.defer()` returns a `hypha::Deferred<Srv>`, a move-only handle any
thread answers later with `reply()` or `refuse()`. Here the camera's handler hands the request to the
worker that owns the device, which answers between two grabs:

```cpp
void setup(hypha::Node& node) override
{
    this->setExposure_ = node.advertiseService<camera_control_msgs::SetExposure>(
        "set_exposure", [this](hypha::Request<camera_control_msgs::SetExposure>& request) {
            this->commands_.push({request->target_exposure, request.defer()});
        });
}

// on the worker, between two grabs
while (auto command = this->commands_.tryPop()) {
    float reached = 0.0F;
    const bool success = this->device_.setExposure(command->target, reached);
    command->reply.reply({.reached_exposure = reached, .success = success});
}
```

A `Deferred` destroyed while it holds a request refuses it (`a deferred request was destroyed
unanswered`): no request ends in silence. After its server is destroyed, or its client left, `reply()`
returns `Disconnected`. An answer from another thread carries the process's framework time and is not
part of a deterministic replay.

### 8.2 A client in a node

```cpp
auto trigger = node.serviceClient<std_srvs::SetBool>(hypha::Service{"/localization/ekf/trigger_node"});
if (trigger.call({.data = true}, [](const hypha::CallResult<std_srvs::SetBool>& r) {
        if (!r.ok() || !r.response().success) HYPHA_LOG_WARN("trigger_node: {}", hypha::describe(r));
    }) != hypha::CallStatus::Sent) {
    HYPHA_LOG_WARN("trigger_node: not sent");
}
```

`call()` sends at once and returns a `[[nodiscard]] hypha::CallStatus`: `Sent`, or a local refusal
that sends and calls nothing: `Busy` (the client has its limit of calls in flight, including calls the
server still holds after they timed out) or `TooLarge` (above the bound). After `Sent` the callback
runs exactly once with the outcome, on the run thread at a later turn, never inside `call()`:

| `CallOutcome` | When | `mayHaveRun()` | `safeToRetry()` |
|---|---|---|---|
| `Answered` | the server replied; `ok()` and `response()` | yes | no |
| `Refused` | `refuse()`, a request left unanswered, a `Deferred` destroyed unanswered, a response above its bound | no | yes |
| `NoServer` | no live server of the stream until the timeout | no | yes |
| `NotSent` | nothing was sent: the transport refused the request, or a blocking call was `Busy` or `TooLarge` | no | yes |
| `ServerLost` | the server exited, crashed or was destroyed; its handler threw; its answer did not verify | yes | no |
| `Timeout` | no answer within the timeout | yes | no |

- `describe(result)` gives the outcome as one line: `refused: not armed`, `timeout after 1 s (policy
  defaultService)`.
- A client may call before its server exists: the call waits in one of its slots, is sent when a
  server appears, and ends `NoServer` at its deadline. `ready()` (ROS 1 `exists()`) says whether a
  live server exists now; `pending()` whether an outcome is still to come.
- Destroying the client drops its in-flight outcomes uncalled.
- The timeout is the policy's (1 s for `defaultService`). `hypha::timeout(d)` as the last argument of
  `serviceClient()` overrides it in code, and `timeout_ms` in the client's table overrides both:

```toml
[ports.clients.trigger]
service = "/localization/ekf/trigger_node"
timeout_ms = 2000
outstanding = 1                    # calls in flight, at most the policy's
```

### 8.3 A blocking client: a worker thread, a test, a tool

```cpp
auto takeoff = node.serviceClient<std_srvs::Trigger>(hypha::Service{"/uav1/uav_manager/takeoff"}, hypha::blocking);
if (!takeoff.waitForServer(hypha::Duration::fromSeconds(5))) { /* no server */ }
const hypha::CallResult<std_srvs::Trigger> r = takeoff.callAndWait({});
```

A blocking client is attached to no `WaitSet`: `callAndWait()` waits on the calling thread and returns
the outcome; a call refused before sending reads `NotSent`. On the run thread, in any callback,
`callAndWait()` and `waitForServer()` throw `InvalidState`, since that thread must deliver the answer;
the message says to use `call(request, onResult)`. The library starts no thread for it.

From the shell, `hypha service call <service> '<toml>'` (`'data = true'`) is the operator's `rosservice
call`. A server registers its `.srv` definitions when created, so the tool encodes the request, prints
the response, and exits with a code per outcome ([tools.md](tools.md), `hypha service`). Every client,
the tool's included, logs each finished call as a `hypha_msgs/ServiceCall` on `<service>/calls` while
someone reads it (`hypha topic echo /camera/set_enabled/calls`; recorded by `hypha record`). A record
carries the first 16 KiB of request and response, their lengths and a `truncated` flag, so logging
needs no payload bound.

### 8.4 Limits, and services in a cycle

A service has a policy as a topic does (section 7): a `hypha::ServicePolicy`, mapped to the type by
`hypha::ServicePolicyOf<Srv>` in a header both ends include. The default, `defaultService`, admits 8
clients (and 3 more for tools), 2 calls in flight each, 4 KiB each way, and a 1 s timeout. A request
above its bound is `TooLarge` at `call()`; a response above it gives the server `TooLarge` from
`reply()` and the client `Refused` with the sizes. Map-sized data goes on a latched topic; a service
carries a small request and its acknowledgement.

### 8.5 A server and a client in a cycle

In a cycle a service is a port (`hypha/cycle_service.h`, `CycleNode::Requests`, `CycleNode::Calls`).
Requests are an input of the server's steps and calls an output of the client's, so a service changes
nothing between two steps and a replay repeats it exactly (FSR-44).

```cpp
class ControlManager final : public hypha::CycleNode {
    Requests<mrs_msgs::String>  switchTracker{*this, "switch_tracker"};  // [ports.servers.switch_tracker]
    Calls<std_srvs::SetBool>    arming{*this, "arming"};                  // [ports.clients.arming]
    Arrivals<nav_msgs::Odometry> odometry{*this, "odometry"};

    void step(const hypha::CycleTime&) override {
        for (hypha::Request<mrs_msgs::String>& request : this->switchTracker.requests()) {
            request.reply({.success = this->select(request->value), .message = "switched"});
        }
        if (auto result = this->arming.result()) {          // the call of an earlier step
            if (!result->ok()) HYPHA_LOG_WARN("arming: %s", hypha::describe(*result).c_str());
        }
        if (this->mustDisarm() && this->arming.call({.data = false}) != hypha::CallStatus::Sent) {
            HYPHA_LOG_WARN("arming: %s", hypha::toString(this->arming.lastStatus()));
        }
    }
};
```

- **Requests.** `requests()` gives the step at most `per_step` requests of the server's table (default
  8); a second call in the same step gives the same ones. Each is answered by `reply()`, `refuse()` or
  `defer()`, and the answer leaves after the step, with its outputs. A request read and left alone is
  refused after the step. A step that never calls `requests()` reads none: they wait for a later step,
  bounded by the client's timeout. A failed step sends no output and no answer: the requests it read
  end `ServerLost` (`server step failed`), since the step may have changed state.
- **Deferred.** A `Deferred<Srv>` (from `defer()` in a step or from a plain server's handler) answered
  during a step of any cycle of the node leaves after that step, and `reply()` returns `Ok` at once. If
  that step then fails, the client reads `ServerLost` (`server step failed`) and the `Deferred` is used
  up. An answer that cannot leave after the step is logged. Answered on another thread or in a plain
  callback, it leaves at once. A chain of calls is a `hypha::Sequence` (section 8.6).
- **Calls.** `call()` stages a call: `Sent`, `Busy` (the port has `outstanding` calls in flight,
  default 1, this step's included) or `TooLarge`. A sent call leaves after the step, stamped with its
  time. A failed step sends none: each call it staged ends `NotSent`, which the next step reads.
- **Outcomes.** `result()` gives the next outcome the step holds, `results()` all of them: the
  outcomes of earlier steps' calls. An outcome a successful step does not read is dropped after it,
  logged once and counted (`unreadResults()`), so fire-and-forget does not wedge the port; a failed step
  passes its unread outcomes to the next step. Timeouts count in framework time: a call of a step at S
  times out at the first step at or after S + timeout.
- **Replay.** On simulated time a step at T holds the requests and outcomes stamped before T, requests
  ordered by stamp, client node, client port and sequence. A request is served at the server's first
  step after the client's, and its answer read at the client's first step after that, whatever
  `--chunk` or turn order, also for two cycles of one `WaitSet` (execution-model.md §6.6 item 5). Cycles
  of two `WaitSet`s of one node are not held against each other: logged, and not deterministic. A
  triggered server reads a request at its next trigger after it, so a server triggered only by its
  client's topic serves the client's call of step `t` at the client's next trigger (the trigger at `t`
  and the request share a stamp, and requests are cut before it).
- **Outside the replay.** A request from `hypha service call` or a plain node is served at the first
  step after it came, and the answer of a server that reports no progress is read at the first step
  after it came, whatever the stamps: not deterministic, logged once.
- A step that fails on replay fidelity (a lost input) has already sent its answers and calls, as it
  sent its outputs.
- A `testing::StepHarness` drives these ports with the test as client and server, by the same rules
  (section 5.4, "Test a step without a bus").

### 8.6 A chain of calls: `hypha::Sequence`

A callback that calls one service after another and then replies (MRS `uav_manager`'s takeoff,
`docs/design/native-services.md` §8.2) is a `hypha::Task`, a C++ coroutine that a `hypha::Sequence`
runs over steps (`hypha/sequence.h`, included by `hypha/cycle_node.h`). It reads in the order of the ROS
callback, and each failure is a value the code handles.

```cpp
class UavManager final : public hypha::CycleNode {
  public:
    Requests<std_srvs::Trigger> takeoff{*this, "takeoff"};
    Calls<mrs_msgs::String>     switchController{*this, "switch_controller"};
    Calls<mrs_msgs::String>     switchTracker{*this, "switch_tracker"};
    Calls<mrs_msgs::Vec1>       trackerTakeoff{*this, "tracker_takeoff"};
    Calls<std_srvs::SetBool>    toggleOutput{*this, "toggle_output"};
    Calls<std_srvs::SetBool>    arming{*this, "arming"};
    Calls<std_srvs::Trigger>    eland{*this, "eland"};

    void step(const hypha::CycleTime& time) override {
        for (hypha::Request<std_srvs::Trigger>& request : this->takeoff.requests()) {
            if (this->takeoffSeq_.running()) {
                request.reply({.success = false, .message = "takeoff in progress"});
            } else {
                this->takeoffSeq_.start(request.defer(), this->runTakeoff());
            }
        }
        this->takeoffSeq_.advance(time);  // in every step while it runs
    }

  private:
    hypha::Task<std_srvs::Trigger::Response> runTakeoff() {
        auto a = co_await hypha::call(this->switchController, {.value = "MpcController"});
        if (!succeeded(a)) co_return co_await this->disarm("could not activate the controller", a);
        auto b = co_await hypha::call(this->switchTracker, {.value = "LandoffTracker"});
        if (!succeeded(b)) co_return co_await this->disarm("could not activate the tracker", b);
        co_await hypha::sleepFor(hypha::Duration::fromMillis(300));  // the motors ramp up
        auto c = co_await hypha::call(this->trackerTakeoff, {.goal = this->takeoffHeight_});
        if (!succeeded(c)) {
            auto e = co_await hypha::call(this->eland, {});
            co_return {.success = false,
                       .message = "takeoff was not successful (" + c.description() + "); eland: "
                                  + e.description()};
        }
        co_return {.success = true, .message = "taking off"};
    }

    /// Turn the output off, then disarm, and fail the takeoff with the reason.
    template <typename Srv>
    hypha::Task<std_srvs::Trigger::Response> disarm(std::string what, hypha::CallResult<Srv> why) {
        co_await hypha::call(this->toggleOutput, {.data = false});
        co_await hypha::call(this->arming, {.data = false});
        co_return {.success = false, .message = what + " (" + why.description() + ")"};
    }

    template <typename Srv>
    static bool succeeded(const hypha::CallResult<Srv>& r) { return r.ok() && r.response().success; }

    hypha::Sequence<std_srvs::Trigger> takeoffSeq_;
    double                             takeoffHeight_{1.5};
};
```

- `co_await hypha::call(port, request)` stages the call as `port.call()` does and resumes with its
  `CallResult` in the first step that reads the outcome (answered, refused, no server, not sent, server
  lost, timeout). A `Busy` or `TooLarge` call resumes at once as `NotSent` with that status.
- `co_await hypha::sleepFor(d)` resumes at the first `advance()` at least `d` of step time later;
  `co_await hypha::until(pred)` once `pred()` holds, checked when reached and at each `advance()`. A Task
  may `co_await` another Task (`disarm()` above).
- `start(request.defer(), task)` begins the Task at the next `advance(time)`, which runs it until it
  waits on something not ready. Its `co_return` answers the request, which leaves after the step.
  `hypha::Sequence<>` runs a Task that answers nothing, such as work after the answer. `running()`,
  `waitingFor()` (`the outcome of a call to 'switch_tracker'`, `the end of a sleep`, `a condition`) and
  `lastAnswer()` show where it is.
- A Task or an `advance()` that throws ends the sequence, refuses the request with the reason and
  rethrows: the step fails, and its client reads `ServerLost`. Destroying a running sequence refuses its
  request.
- A step that does not read an outcome drops it, so call `advance()` in every step while the sequence
  runs; otherwise the next `advance()` throws `std::logic_error` naming the port. Read no other outcome
  of a port the sequence waits on, and keep no reference to step-local data or an input's value across
  a `co_await`.
- The sequence follows the step's inputs and time alone, so a replay repeats it. One frame is
  allocated per Task called; `advance()` allocates nothing.

`advance()` takes the step's time, not `hypha::now()`, so a sleep counts in step time under a
`testing::StepHarness` too. `crates/hypha-cxx/test/test_sequence.cpp` tests this takeoff with every
failure branch, and `test_cycle_doc_examples.cpp` compiles the example.

## 9. Parameters

A node reads its one parameter file (the program argument of `runNode`, or the second argument of
`Node`) through `hypha::config()`. `examples/scan_cycle/inc/scan_cycle/scan_cycle.h` reads checked
scalars, `examples/sensor_fusion/src/sensors_main.cpp` a `[[sensors]]` list, and
`examples/mrs/config/mrs_simulator_teleop.toml` includes another file. The file is read once, at start:
there is no parameter server and no mutation API.

A process has one parameter file, whatever its `Node`s and `ConfigContext`s. The first one that names a
file loads it; each later one names no file (to share it) or the same file (to join it). A different
file throws `std::runtime_error` naming both (`hypha: node 'beta' creation failed: InvalidState: the
process already loaded '/etc/a.toml', so '/etc/b.toml' is refused: a process reads one parameter file;
name '/etc/a.toml' or no file to share it`), and so does a file after a first `Node` that named none
(#208). A test's client `Node` therefore needs no file of its own.

```cpp
const auto settings = hypha::config();                       // config("controller") scopes reads
const double rate = settings.required<double>("rate_hz", hypha::constraints::positive());
const double gain = settings.optional<double>("gain", 1.0);  // the fallback only when absent
for (const auto sensor : settings.list("sensors")) {
    const auto topic = sensor.required<std::string>("topic", hypha::constraints::nonempty());
}
```

- **Reads.** `required<T>()` refuses an absent or malformed value. `optional<T>()` gives `std::nullopt`
  or the fallback only for an absent key; a present value that does not read as `T` throws
  `ParameterError`. The fallback is validated too. Integers read by width and signedness; out of range
  throws. `constraints::range(min, max)` is inclusive: two integer bounds compare an integer exactly
  (`required<uint8_t>("channels", constraints::range(1, 8))`); a floating-point bound gives a
  floating-point range, which also refuses non-finite values. Relations between fields are the
  application's to check.
- **Types.** A value keeps its TOML type: `"5"` is no number, `5` no text, `1` no `bool`. A whole number
  reads as a floating-point type. `${NAME}` always gives text.
- **One of a list.** `constraints::oneOf({4800, 9600, 19200})` accepts the listed values
  (`required<uint32_t>("baud_rate", constraints::oneOf({4800, 9600, 19200}))`, or `oneOf({"fast",
  "slow"})`), numbers compared exactly whatever the width. It refuses with the list: `hypha: parameter
  'baud_rate' must be one of 4800, 9600, 19200`. `optional<T>(key, fallback, oneOf(...))` checks the
  fallback too.
- **Lists.** An array is a list of scalars or of tables (`[[name]]`), never mixed and never a list of
  lists (a matrix is a flat list, below); an empty array has length zero. `list(name)` throws for a
  missing or scalar name, `at(index)` past the end. A scalar element reads with the key `""`, a table
  element by field, a nested table by dotted name (`limits.max`). Elements read as `bool`, `int64_t`,
  `double` or `std::string`. A list name may not equal a scalar name or be a dotted prefix of one
  (`InvalidArgument`). Errors name the entry: `sensors[2].topic`.
- **Lists in a list entry.** An entry of a table list may hold arrays or `[[sensors.points]]` tables,
  read through the entry: `for (const auto sensor : cfg.list("sensors")) { auto w =
  sensor.requiredArray<double>("weights"); for (const auto point : sensor.list("points")) ... }`.
  Errors name them (`sensors[1].weights[2]`), and `unreadKeys()` lists them as `sensors[1].weights`. The
  C ABI reads them by entry name: `hypha_config_list_len(config, "sensors[1].weights")`, and
  `hypha_config_value` a scalar of an entry the same way.
- **Arrays of scalars.** `cfg.requiredArray<T>(key)` returns `std::vector<T>`;
  `cfg.requiredArray<T, N>(key)` returns `std::array<T, N>` and refuses another length.
  `optionalArray<T>` and `optionalArray<T, N>` return a `std::optional`, empty only when the key is
  absent. `T` is any type `required<T>` reads, with the same conversion and range check per element. A
  non-array, an element of the wrong type, an array of tables or a wrong length throws `ParameterError`
  in both forms, naming the key and the element's index (`hypha: parameter 'voxel_weights[2]': expected
  a number, found "x"`; `hypha: parameter 'extrinsic_t': expected 3 elements, found 4`). An empty array
  is an empty vector (a length error for N > 0). Reading an array marks it read.
- **One value or a list.** `cfg.requiredScalarOrArray<T>(key)` returns a `std::vector<T>` for a key
  given as one value or an array (`topic = "/a"` or `topic = ["/a", "/b"]`), the ROS parameter that
  takes one or many; `optionalScalarOrArray<T>` is empty only when absent. An array reads as
  `requiredArray<T>()`; another type throws `ParameterError` naming the key. The manifest records the
  value as given.
- **A matrix.** `cfg.requiredMatrix<double>("mixer", rows, cols)` reads the flat row-major list a ROS
  matrix parameter is (`mixer = [1, 0, 0, 1, 0.5, 0.5]`) into a `hypha::Matrix<T>` (`rows`, `cols`,
  `values`, `at(row, col)`; `<hypha/config_matrix.h>`, included by `config.h`). A 0 for `rows` or
  `cols` takes that size from the length: `requiredMatrix<double>("mixer", 0, 6)` is any number of rows
  of 6. A length that does not fit throws `ParameterError`: `hypha: parameter 'mixer': expected 4 x 2 =
  8 elements, found 6`. A TOML list of lists (`[[1, 0], [0, 1]]`) is not read; write the flat list, as
  ROS does. With Eigen,
  `Eigen::Map<const Eigen::Matrix<double, -1, -1, Eigen::RowMajor>>(m.values.data(), m.rows, m.cols)`
  views it without a copy.
- **Tables under a scope.** `cfg.tables()` is a `std::vector<ConfigView>`, one per table directly under
  the scope, sorted by name; `key()` is its name. Where ROS reads a parameter namespace, a node reads
  its tables: `for (const auto& input : hypha::config("ports.inputs").tables()) {
  priority[input.key()] = input.required<int>("priority"); }`, so the tables are the list and a test
  file with other inputs works with no second list of names. A key that holds a value or list is no
  table, and a table with no key is not there. It reads nothing: a key the node does not read stays
  unread. It works in a list entry too (`sensor.tables()`). C ABI: `hypha_config_table_names(config,
  prefix, ...)`, names joined by `'\n'` (`abi.revision` 14).
- **Environment.** Any string value (list elements, `[log]` and `include` too) may hold `${NAME}`,
  expanded when its file is read. An unset or empty variable fails node creation: ``could not load TOML
  configuration `<file>` at `<key>`: environment variable `<NAME>` is unset`` (or `is empty`). `$NAME`
  stays literal. An empty variable does not switch off an optional port: its topic is refused.
- **Port tables.** A bare string names `[ports.inputs.<name>]` or `[ports.outputs.<name>]`, which hold
  `topic` (section 5.3); an input and an output may share a name. A plain entry `speed = "/odom_speed"`
  is `InvalidArgument`. A table may hold a key of the application's, such as each input's `priority`
  in a multiplexer; read it with `config("ports.inputs.<name>")`, so no second table names every input.
  A cycle input accepts such a key only if the application read it before `start()` (constructor or
  `setup()`; `started()` comes after), else `start()` fails as unknown key (section 5.4). Framework keys
  are never application keys: `silence_mss`, or `stagnation_ms` without `setAlive()`, is refused even
  when read. An output's table, or a subscriber's of an `ApplicationNode`, has no check at `start()`: a
  key never read there is an unread key (below).
- **Reserved names.** `node`, `ports` and `log` are the framework's: `node.name` (section 2), the port
  tables, and `[log]` (section 11), whose keys the parameter API does not return. The top-level keys
  `include` (below) and `unused` are not parameters either. Simulated time is `HYPHA_USE_SIM_TIME` only;
  a `use_sim_time` key is ordinary.
- **Unread keys.** `unreadKeys()` lists, sorted, the keys under a view that nothing read (`node.name`
  excluded; a list field as `list[index].field`). `runNode` exits 78 if one remains after `setup()` and
  cycle start; a custom `main()` checks `hypha::config().unreadKeys()` itself. A value chosen in code
  goes into the manifest with `hypha::recordConfig(key, value)`.
- **Keys set on purpose and not read (#119).** Two ways keep a key out of `unreadKeys()` without
  deleting it; a typo beside them is still refused. Each becomes a manifest row (`~/introspection`)
  with source `SOURCE_IGNORED` and the reason as value, and is logged once at info.
  - In code, `cfg.ignore(keyOrPrefix, reason)` marks a key, table or list entry as read by a branch the
    code did not take (`config().ignore("model_params", "no TensorRT in this build")`). A key the file
    does not set is fine. An empty key or reason throws `ParameterError`.
  - In the file, a top-level `unused = ["pose_measurement.pose_measure_uncertainty_time"]` lists keys
    or tables a vendored file sets and this node does not read. Every file of a composition may list
    its own. An entry the composed file does not set (a typo) refuses the file at load, so the list
    cannot go stale.

  The check is in the C ABI layer (`hypha_config_ignore`; `hypha_config_unread_keys` leaves ignored
  keys out), so every SDK has it.
- **Another TOML file.** `hypha::ConfigFile file(path)` loads a TOML file with the parameter-file loader
  (`include`, `${NAME}`), apart from the node's configuration; `file.view(prefix)` reads it with every
  `ConfigView` reader. `path()` resolves against that file's directory, `unreadKeys()` lists its unread
  keys, and its reads are not node parameters (no manifest rows). A missing or non-TOML file throws
  `ConfigError` (the loader logs the reason and line). Views borrow the file. C ABI:
  `hypha_config_create` and the configuration readers.
- **An optional file name.** `cfg.optionalPath(key)` is `path(key)`, or no value when `key` is not set;
  a set but empty key throws as `path()` does.
- **Camera calibration (#265, #316).** `<hypha/camera_info.h>` reads a calibration into a
  `sensor_msgs::CameraInfo`, from a table of the parameter file or from its own file, both TOML with the
  keys of the ROS calibration file:

  ```toml
  image_width = 640
  image_height = 480
  camera_name = "narrow_stereo"           # optional
  distortion_model = "plumb_bob"
  distortion_coefficients = [-0.3635, 0.1619, -0.0001, 0.0003, 0.0]   # any length
  camera_matrix = [                       # 3 x 3, row-major (ROS's K)
      430.215, 0.0, 306.691,
      0.0, 430.532, 227.225,
      0.0, 0.0, 1.0,
  ]
  rectification_matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]   # 3 x 3 (R)
  projection_matrix = [432.0, 0.0, 306.0, 0.0, 0.0, 432.0, 227.0, 0.0, 0.0, 0.0, 1.0, 0.0]  # 3 x 4 (P)
  binning_x = 0                           # optional, 0 when absent
  binning_y = 0                           # optional, 0 when absent
  [roi]                                   # optional; each key optional, 0 or false
  x_offset = 0
  y_offset = 0
  height = 0
  width = 0
  do_rectify = false
  ```

  - A matrix is the flat row-major array (the `data` of the ROS file), checked against its shape.
  - In the node's own file the keys sit in `[camera_info]` (and `[camera_info.roi]`), read with
    `hypha::readCameraInfo(hypha::config("camera_info"))`. A file of its own, which `set_camera_info`
    may rewrite at run time, is `hypha::readCameraInfo(hypha::config().path("camera_info_url"),
    &cameraName)`. `readCameraInfoIfPresent(path)` returns no value when nothing is at `path`, so the
    camera runs uncalibrated as with `camera_info_manager` (the driver logs it), and
    `config().optionalPath("camera_info_url")` makes the key itself optional.
  - `hypha::writeCameraInfo(path, info, cameraName)` writes the same TOML (to `<path>.tmp`, renamed over
    `path`, creating missing directories) and refuses a calibration the reader would refuse; what it
    writes reads back equal. Reading uses the parameter-file loader through `ConfigFile`; the SDK holds
    no parser.
  - It is strict, like a bad parameter: a missing or unknown key (a typo, or the ROS `rows`/`cols`/`data`
    layout, which the message says to flatten), a size of 0, a non-finite number, an empty
    `distortion_model` or a matrix of the wrong length throws `ConfigError` naming the file or table and
    the key, so a bad calibration read in the constructor or `setup()` exits 78: `hypha: camera
    calibration 'left.toml': camera_matrix: expected 3 x 3 = 9 elements, found 8`.
  - The result's `header` is empty: the driver names the frame and stamps each message. The ROS
    calibration YAML is not read: convert it once by hand
    ([ros1-migration-guide.md](ros1-migration-guide.md), the `camera_info_manager` row).
    `crates/hypha-cxx/test/test_camera_info.cpp` tests it.
- **Porting.** A ROS 1 list parameter, with the fixed-size form:
  [ros1-migration-guide.md](ros1-migration-guide.md#108-a-list-of-scalars-in-the-parameter-file).
- **Without a bus.** A `hypha::ConfigContext` serves `hypha::config()` in a test
  (`examples/mrs/test/test_plugin_host.cpp`); `hypha param validate` checks a file.

### Composing files

`include = ["system.toml"]` at the top level merges files in the listed order; it is not a parameter. A
relative path in `include`, in `path(key)` or in a `[log]` sink resolves against the directory of the
file that wrote it. Tables merge key by key. The root file overrides its includes: its scalar or list
replaces the key at any depth, with one debug line `hypha: <key> from <root file> overrides <included
file>`. One file may appear in two branches with disjoint keys. Node creation fails with
`InvalidArgument`, naming file and key, for:

- a file that cannot be read, or an `include` that is not an array of strings;
- a scalar or list key set by two included files, or a key that changes kind between files (root file
  included);
- an include cycle, a chain deeper than 16, or more than 256 included files in one load (a file included
  twice counts twice).

Reference: `hypha/config.h`, `hypha/config_list.h` and `hypha/config_matrix.h`, which it includes.

### 9.1 Node introspection

`hypha::introspect()` returns the node as a `hypha_msgs::NodeIntrospection`: its identity, the
configuration rows, and every live endpoint, cycle and cycle input binding with its effective settings.
`hypha::nodeState()` returns the `hypha_msgs::NodeState` that `runNode` publishes on `~/state`. Its
`state` is `WAITING` while a required input never arrived, else `DEGRADED` while one is not ok or an
application check is at `ERROR` or `STALE` (`hypha::reportChecks`, section 2.7), else `INPUTS_USABLE`.

Each `CycleState` row carries the step-time distribution, in host monotonic nanoseconds: median, 90th
and 99th percentile of every step since commit (`step_p50_nanos`, `step_p90_nanos`, `step_p99_nanos`),
the same for the last 10 to 20 seconds with their longest (`recent_step_*`), and `last_step_nanos` and
`max_step_nanos`. A quantile is the upper edge of a bucket of a fixed histogram (16 buckets per power of
two): at most 1/16 above the true value, never below. `hypha topic delay <input> <output>` gives the
time from an input to an output instead ([tools.md](tools.md)).

A program with its own `main()` publishes both itself. `hypha node` and `hypha top`
([tools.md](tools.md)) show them. The fields are in
`crates/hypha_msgs/msg/hypha_msgs/NodeIntrospection.msg`, `NodeState.msg` and the row messages; row
rules and counters in `hypha/introspection.h`. C ABI: `hypha_node_introspect` and `hypha_node_state`
return `TooLarge` with the needed length.

## 10. Coordinate frame transforms (TF)

TF is a convention on ordinary topics: `/tf` and `/tf_static` carry `tf2_msgs/TFMessage`.
`hypha::TransformBroadcaster` and `hypha::StaticTransformBroadcaster` (section 10.2) send on them and
refuse a transform every buffer would refuse. A plain publisher or `Output` of that type sends without
the check, as `examples/mrs/managers/transform_manager.cpp` does, and the buffers still refuse what is
invalid.

- Dynamic transforms go to `/tf` when they change; transforms that share a stamp go in one message.
- Static transforms go to a `latched` `/tf_static`. A late subscriber gets only the last message, so a
  static broadcaster keeps every static transform it owns, keyed by child frame, and sends the whole set
  each time (`StaticTransformBroadcaster` does).
- The stamp is the time the transform describes; a listener interpolates between stamps. A rotation is
  a unit quaternion (`w = 1` for none).

### 10.1 Looking transforms up in a cycle

A cycle reads TF with a `hypha::Transforms` port (`<hypha/cycle_transforms.h>`, not in
`hypha/hypha.h`). The step looks up; nothing waits. `examples/mrs/host/include/mrs_lib/transformer.h`
wraps the port.

```cpp
hypha::Transforms tf{*this};                               // a member of the CycleNode
const auto found = this->tf.lookupLatest("map", "base_link");  // in step(): base_link in map
```

Buffer, clock steps, losses: [execution-model.md](execution-model.md) §6.7. `tf.clear()` in a step
forgets the dynamic transforms when the step returns, as tf2's `Buffer::clear()`, and keeps the static
ones (§5.4, a reset).

### 10.2 Looking transforms up in a plain node, and sending transforms

A node without a cycle reads TF with a `hypha::TransformBuffer` (`<hypha/transform_buffer.h>`, not in
`hypha/hypha.h`). The node's run ingests every `/tf` and `/tf_static` message into the buffer, on host
and simulated time alike. The lookups are those of `Transforms` and return the same
`std::expected<geometry_msgs::TransformStamped, hypha::TransformError>`; `hypha::name(error)` gives the
error's name for a log line (`"UnknownFrame"`).

```cpp
// or {node, {.tf = "/tf", .tfStatic = "/tf_static"}, hypha::Duration::fromSeconds(10)}
hypha::TransformBuffer tf{node};   // a member: `TransformBuffer tf_;` then `tf_ = TransformBuffer(node);`
const std::optional<hypha::Time> stamp = hypha::fromRosTime(scan.header.stamp);
if (stamp.has_value()) {
    const auto found = tf.lookup("map", "lidar", *stamp);   // in a callback or a worker thread
}
const auto latest = tf.lookupLatest("map", "base_link");

hypha::TransformBroadcaster broadcaster{node};          // /tf, the tf policy
if (!broadcaster.send(mapToBase)) {                      // a NaN pose is refused, not sent
    HYPHA_LOG_ERROR("{}", broadcaster.lastRefusal());    // names the frames and the reason
}
hypha::StaticTransformBroadcaster statics{node};        // latched /tf_static
const bool sent = statics.send(baseToLidar);            // keeps it, resends the whole set
```

- **Threads.** `lookup()` and `lookupLatest()` are `const`, never wait, and may be called from any
  thread while the buffer lives, also while `node.run()` runs on another thread. Create and destroy the
  buffer on the thread that configures the node, not while the node runs elsewhere. Nothing is ingested
  unless the node runs. The cache (default 10 s) must be positive.
- **No lookup waits for a transform, by design (#91).** The buffer fills only on the run thread, so a
  callback that waited would block every other callback and, in a replay, the clock it waits for. A
  missing transform is an error the caller handles. ROS 2's `lookupTransform` with a timeout and
  `waitForTransform` have no counterpart: hold the message and retry in a later callback once `/tf`
  brought the transform (what `tf2_ros::MessageFilter` does), or drop it.
- **Policy.** The buffer declares the `tf` policy on both topics, as `Transforms` does. A topic that a
  raw endpoint or tool created first with `default` refuses it with `QosMismatch`, and the constructor
  throws `ConfigError`.
- A `/tf_static` sample latched before the buffer existed is found once the node has run. A refused
  destroy (the node runs on another thread) is logged, not thrown, and the buffer stays attached until
  the node is destroyed.
- **A refused transform (#123).** A lookup that fails on a frame whose newest transform the buffer
  refused (not finite, not a unit quaternion, a stamp that is no time, a second parent) returns
  `TransformError::Refused` instead of `UnknownFrame`, `NotConnected` or `AfterBuffer`: the producer is
  broken, not late. The frame is the target, the source or a frame above either; a later accepted
  transform of that frame clears it. The buffer's error line, at most once a second, names the reason,
  and the node metrics `tf.refused` and `tf.undecodable` count every refused transform and undecodable
  message for good. `Transforms` answers the same.
- **Both broadcasters check what they send**, with the check every buffer applies on ingest
  (`hypha_transform_check`, below the C ABI): an empty frame name, a stamp that is no time (`nsecs` of a
  second or more; not read for a static transform), a non-finite value, a non-unit quaternion. A refused
  send sends and keeps nothing of its message and returns false; `lastPublishStatus()` is
  `InvalidArgument` and `lastRefusal()` names child, parent and reason (`hypha: transform refused:
  'base_link' in 'map': ...`). A second parent or a cycle needs the other transforms, so only a buffer
  refuses those. Any other false is the publish's, as for `Publisher::publish`, with `lastRefusal()`
  empty.
- **Sending.** `TransformBroadcaster` publishes each `send()` (one `TransformStamped`, or a
  `std::vector` as one message) on `/tf` with the `tf` policy; an empty vector sends nothing and returns
  true. `StaticTransformBroadcaster` keeps every transform sent through it by child frame (a later one
  replaces the earlier in place) and publishes one latched message with the whole set on each `send()`.
  Each is an endpoint of one thread at a time. Several static broadcasters may share `/tf_static`; each
  sends its own set.

## 11. Logging

Log with the `HYPHA_LOG_*` macros of `<hypha_log/hypha_log.h>`, one `HYPHA_LOG_MODULE_INIT("name")` per
translation unit, and set levels and files in the `[log]` table. `examples/speed/src/speed_monitor.cpp`
uses the throttled and `_WHEN` forms; `examples/scan_cycle/config/control.toml` has a rotating file
sink. The macro families and their evaluation rules are in the comment of `hypha_log/hypha_log.h`.

The engine is in `libhypha_ffi`. It is synchronous, with no thread and no buffer, so a slow disk delays
the logging thread and nothing needs cleaning up. Rust code logs through the `log` crate, which node
creation routes to the same engine; iceoryx2 lines arrive at `Warn` and above under targets such as
`iceoryx2/service`. The counters are the `log.*` metrics (section 2.4).

```toml
[log]
level = "info"                 # modules without an entry; default "debug"
console = true                 # standard error; default true
console_level = "info"         # default "trace"
slow_callback_ms = 50          # 0 to 3600000; default: the period of a timer or a cycle, 10 for a subscription
[log.sink.main]                # any number of named file sinks
path = "log/scan_cycle.log"    # required; relative to this file
level = "debug"                # default "info"
rotation_mb = 5                # 1 to 1024 MiB, default 5
rotation_count = 3             # 1 to 100 kept files, default 3
enabled = true                 # default true
[log.module."localisation"]
level = "debug"                # default log.level; enabled = false is "off"
```

`slow_callback_ms` belongs to the node, not the process, whatever the first table says: how long a
callback may take before the node warns. The line names the callback, its time and the budget, at most
once a second per callback: `hypha: callback of topic '/sensing/lidar/points' took 46 ms, budget 10 ms,
9 warnings suppressed` (`timer 100 ms` and `cycle 'ekf'` for the others). Without the key a timer or
cycle is judged against its own period (a 10 Hz callback that takes 45 ms logs nothing) and a
subscription against 10 ms; a value replaces that for every callback of the node, and 0 writes no line.
`node.slow_callbacks` (section 2.4) counts either way. An invalid value refuses the file at load (exit
78), like the rest of the table. Node creation reads it; it needs no C function. Rules:
execution-model.md section 5.

Levels are `trace`, `debug`, `info`, `warn`, `error`, `fatal`, `off`. A line reaches a target at or
above both its module level and the target level. The parameter file is the only configuration (no
environment variable). Node creation applies it before the transport starts, and an invalid table fails
creation with `InvalidArgument`. One process has one logging configuration, set by the first table
([engineering-traps.md](engineering-traps.md)).

A line reads `[2026-10-06T12:00:05.123Z] [sim 412.250000] [WARN] [scan_cycle] text`: UTC wall time,
then, on simulated time only, framework time (in a step, the step's time). A file rolls to `<path>.1`
and on after the line that passed `rotation_mb`. Logging never stops a node; on Linux the console never
blocks a step and counts the lines nobody read. Validation, rotation and console rules:
[interface-specification.md](interface-specification.md#logging-engine).

## 12. Patterns for event-driven sensor pipelines

A sensor pipeline is a chain of processes; each stage is a callback subscriber that publishes (section
5.2) or a triggered cycle (section 5.4). `examples/large_payload` sends megabyte frames on raw loans;
`examples/bench` runs a point cloud, an image and a 1 kHz control topic in one process.

### 12.1 Every publisher notifies; the reader decides

A sample is sent before its wake. A wake carries no sample and no count, several sends may coalesce, and
the reader drains its queue. Which readers wake: [execution-model.md](execution-model.md) §7. A latched
topic wakes a late joiner with the retained sample; a join with no retained sample wakes nobody.

### 12.2 Callbacks and thread ownership

The thread in `run()` runs every callback, once per queued sample. A publish in a callback wakes the
next process; it never runs downstream code on its own stack. One endpoint belongs to one thread at a
time. Destroy the `Node` after `run()` returns. A worker thread that publishes, and
`Timer::missedCount()`: [ros1-migration-guide.md](ros1-migration-guide.md#106-threads).

### 12.3 Failure and ordering contract

- One subscription's surviving samples arrive in FIFO order. Order across topics or processes is not
  defined, so correlated data carries its acquisition stamp.
- A failed notification after a sent sample degrades a diagnostic; it is not a failed publish, and a
  retry would duplicate a visible sample. It is logged and counted (`transport.failed_notifications`);
  the sample waits for the topic's next notification.
- An undecodable cycle input sample ([features.md](features.md)) logs `hypha: cycle '<cycle>' input
  '<port>' (<topic>): a sample did not decode as <type>; dropped (<n> so far)`. A `Latest` keeps its
  earlier value; an `Arrivals` delivers the others.
- A shutdown or an endpoint change is reported before queued data, so a busy topic cannot starve a stop.
  Each subscriber has its own bounded queue: a stalled consumer paces nobody, and a loss the transport
  cannot count is reported as not available.

### 12.4 Shared-memory lifetime

A raw callback's byte view lives until the callback returns; a typed subscriber decodes into an owning
message. A wake does not synchronize streams: a deskew stage keeps its own bounded IMU history and,
without coverage, sends an explicit rejected output, never an uncorrected scan.

### 12.5 Sizing and validation

Depth one fits only when the newest frame replaces all older work; otherwise size for the measured
jitter and stay bounded, so overload is counted loss. Validate with separate processes:
publish-to-callback latency at p50 and p99 (the target is under 500 µs at p99), burst loss, idle CPU,
shutdown latency, one stalled consumer.

## 13. Complete API reference

The reference is the doc comments of the public headers; no build runs Doxygen (`docs/Doxyfile`
renders them for a reader who has it). Every result type prints as a name: `Status::name()`, and
`hypha::name()` of a `TransformError`, an `InputVerdict` or a `StopReason`; a value outside the enum
reads `"Unknown"`. `<hypha/hypha.h>` includes all of them except `hypha/cycle_transforms.h`,
`hypha/transform_buffer.h`, `hypha/testing.h` and the three `hypha/diagnostic_*.h`.

| Header | Holds |
|---|---|
| `hypha/node.h` | `Node`, `RunResult`, `StopReason`, `Timer`, `ok`, `nodeName`, `requestStop` |
| `hypha/application_node.h` | `ApplicationNode`, `runNode`, `failRun`, `stopToken`, the exit codes |
| `hypha/worker.h` | `sleepFor`, `sleepUntil`, `Rate`: a worker's sleep and rate, ended by the node's stop |
| `hypha/handover.h` | `Handover<Kinds...>`: the bounded queue from foreign threads to the publishing worker, with counted drop-oldest |
| `hypha/callback.h`, `hypha/topic.h` | `bind<&Class::method>(object)`; `Topic`, `validateTopicName` |
| `hypha/options.h`, `hypha/qos.h` | The option tags; `Qos`, `QosOf`, `defaultQos`, `tfQos`, `imageQos`, `cloudQos` |
| `hypha/message_size.h` | `serializedSize(message)`, `serializedSize(head, loanedBytes)`: the bound of a payload |
| `hypha/publisher.h`, `hypha/subscriber.h` | `Publisher<M>`, `Subscriber<M>`, `MessagePtr<M>` |
| `hypha/cycle.h` | `Hz`, `CycleTime`, `InputVerdict`, `CycleHealth`, `BindingHealth` |
| `hypha/cycle_node.h` | `CycleNode`; it includes `cycle_runtime.h` (`Cycle`), `cycle_latest.h`, `cycle_arrivals.h` (with `cycle_arrivals_merge.h`: `visitArrivalsInOrder`), `cycle_output.h`, `cycle_input.h` (with `cycle_sharing.h`: `sharing`), `cycle_supervision.h`, `cycle_port.h` |
| `hypha/cycle_transforms.h`, `hypha/transform_buffer.h`, `hypha/transform_types.h`, `hypha/testing.h` | `Transforms`; `TransformBuffer`, `StaticTransformBroadcaster`; `TransformError`, `TransformTopics`; `testing::StepHarness` |
| `hypha/raw_api.h`, `hypha/raw.h` | Raw endpoints, `lookupDefinition`, `listNodes`; `RawSample`, `NodeInfo`, `Metric`, `SubscriberMetrics` |
| `hypha/diagnostic_updater.h`, `hypha/diagnostic_status.h`, `hypha/diagnostic_functions.h` | `DiagnosticUpdater`, `DiagnosticTask` and its two kinds; `DiagnosticStatusWrapper`; `FrequencyStatus`, `TimeStampStatus`, `TopicDiagnostic`, `HeaderlessTopicDiagnostic` |
| `hypha/status.h` | `Status`, `ConfigError`, `require` |
| `hypha/config.h` | `config()`, `ConfigView`, `ParameterError`; it includes `config_file.h` (`ConfigContext`, `ConfigFile`), `config_list.h` (`ConfigList`), `config_matrix.h` (`Matrix`) and `constraints.h` (`constraints`) |
| `hypha/camera_info.h` | `readCameraInfo`, `readCameraInfoIfPresent`, `writeCameraInfo`: the calibration in TOML as `sensor_msgs::CameraInfo` |
| `hypha/introspection.h`, `hypha/metrics.h` | `introspect`, `nodeState`, `recordConfig`, `reportChecks`, `HealthCheck`; `metrics`, `metric` |
| `hypha_base/time.h`, `hypha_base/clock.h`, `hypha/message_stamp.h` | `Duration`, `Time`, literals; `now`, `isClockValid`, `steadyNow`, simulated time; `toRosTime`, `rosTimeFromSeconds`, `rosTimeFromNanos`, `fromRosTime` |
| `hypha_base/short_namespace.h` | the alias `hy` of `hypha`; `HYPHA_NO_SHORT_NAMESPACE` leaves it out |
| `hypha_log/hypha_log.h` | The `HYPHA_LOG_*` macros |
| `hypha/hypha_ffi.h` | The C ABI below them all: [interface-specification.md](interface-specification.md) |
