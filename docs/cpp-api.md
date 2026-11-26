# C++ API

A task guide to the C++ SDK: per task, what to write and which program under
[examples/](../examples/README.md) does it. The reference of each class and method is
the doc comment of its public header (section 13 lists the headers).
The run and its timing are in [execution-model.md](execution-model.md), pitfalls in
[engineering-traps.md](engineering-traps.md), the C ABI in
[interface-specification.md](interface-specification.md). `<hypha/hypha.h>` is the
single include of a node; `hypha::Transforms` needs `<hypha/cycle_transforms.h>`
besides (section 10.1), `hypha::TransformBuffer` and `hypha::StaticTransformBroadcaster`
`<hypha/transform_buffer.h>` (section 10.2), and the step harness `<hypha/testing.h>`
(section 5.4).

`hy` is a second name of the namespace `hypha`: `hy::Node`, `hy::Duration` and
`hy::now()` are `hypha::Node`, `hypha::Duration` and `hypha::now()`. It is a namespace
alias, not a namespace of its own, so types, lookup, specializations (`hypha::QosOf`)
and compiler messages keep the name `hypha`. `<hypha/hypha.h>` and the `hypha_base/`
headers declare it (`hypha_base/short_namespace.h`). It takes the global name `hy`: a
program with a global `hy` of its own defines `HYPHA_NO_SHORT_NAMESPACE` before it
includes a hypha header. This guide and hypha's own code write `hypha::`.

| Task                                                                  | Section | Example                                                                                              |
| --------------------------------------------------------------------- | ------- | ---------------------------------------------------------------------------------------------------- |
| A node that steps over its inputs at a rate                           | 1, 5.4  | `examples/speed`, `examples/scan_cycle`                                                          |
| A program with its own `main()` and a bounded run                   | 2       | `examples/sensor_fusion/src/probe_main.cpp`                                                        |
| Time, timers, simulated time                                          | 3       | `examples/sim_twin`                                                                                |
| Publish outside a cycle                                               | 4       | `examples/sim_twin/src/sim_clock.cpp`                                                              |
| Handle each message as it arrives                                     | 5, 5.2  | `examples/sensor_fusion/src/probe_main.cpp`                                                        |
| Optional and advisory inputs                                          | 5.3     | `examples/scan_cycle`                                                                              |
| Triggered cycles, a `/clock` driver, latched outputs, `failRun()` | 5.4     | `examples/mrs`                                                                                     |
| Ports built at run time                                               | 5.5     | `examples/sensor_fusion`                                                                           |
| Raw bytes and loans                                                   | 6       | `examples/large_payload`, `examples/bench`                                                       |
| A service policy for a topic                                          | 7       | `examples/large_payload/inc/large_frame_qos.h`                                                     |
| Request and reply over topics                                         | 8       | `examples/mrs/host/include/mrs_host/service.h`                                                     |
| Parameters and lists                                                  | 9       | `examples/sensor_fusion`, `examples/scan_cycle`                                                  |
| Transforms                                                            | 10      | `examples/mrs/managers/transform_manager.cpp`, `examples/mrs/host/include/mrs_lib/transformer.h` |
| Logging                                                               | 11      | `examples/speed`, `examples/scan_cycle` (the `[log]` table of `config/*.toml`)               |
| Large payloads and sensor pipelines                                   | 12      | `examples/large_payload`, `examples/bench`                                                       |

## 1. Quick start

A node whose work is one step over its inputs is a `hypha::CycleNode`. Its ports
are members, and `step()` is a method. The ports read their topics from the
parameter file (`examples/speed/src/speed_monitor.cpp` is a full one):

```cpp
#include <hypha/hypha.h>
#include <geometry_msgs/Pose.h>
#include <geometry_msgs/Twist.h>

class Follower : public hypha::CycleNode {
    hypha::Latest<geometry_msgs::Pose> pose{*this, "pose"};
    hypha::Output<geometry_msgs::Twist> command{*this, "command"};
    void step(const hypha::CycleTime&) override {
        if (pose.usable()) command.send(follow(*pose));
    }
};

int main(int argc, char** argv) {
    return hypha::runNode<Follower>(argc, argv, "follower");
}
```

`follow` stands for the application's own function. The parameter file is the
one argument of the program:

```toml
[cycle]
rate_hz = 50.0

[ports.inputs.pose]
topic = "/pose"

[ports.outputs.command]
topic = "/cmd_vel"
```

Section 5.4 describes the cycle node. A node whose work is to handle each
message as it arrives uses callback endpoints instead:

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

`hypha::Topic{...}` names a topic that code fixes; a bare string names a port table of
the parameter file (section 9). A default-constructed `hypha::Node` is a handle to the
process session, for code without the `Node` at hand: `hypha::Node().advertise<T>(...)`.
It needs a named `Node` created first; without one the call throws
`... failed: InvalidState: no session exists; create a named hypha::Node first`.
The object form holds the endpoints as members, and `hypha::bind` registers a member
function as the callback:

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
    // run() calls onImu on this thread for every received message.
    return this->node_.run();
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

A `Node` must exist before an endpoint is created; `runNode` creates it for an
application. A node that reacts to messages and runs until stopped is a
`hypha::ApplicationNode` whose `setup(Node&)` creates these endpoints
([ros1-migration-guide.md](ros1-migration-guide.md#102-the-application)); the `main()`
forms above are for bounded probes and tools (section 2). SIGINT and SIGTERM make `ok()`
false and stop the run. The tests
`crates/hypha-cxx/test/test_quick_start_doc_examples.cpp` (callback, publish-only and
README examples) and `test_cycle_doc_examples.cpp` (sections 5.4, 5.5, 10.1) compile them.

### 1.1 Messages

The SDK installs the standard ROS message packages at their ROS 1 path,
`<package>/<Name>.h` (`#include <sensor_msgs/Imu.h>`); hypha's own messages and
`ros_time.h` are in `hypha_msgs/`, the directory of the `hypha_msgs` package. The SDK
owns the directory of each standard package it ships, and an application owns the
directories of the packages it declares: declaring a standard package a second time, from
a directory of its own, stops the configure. A process cannot use hypha messages and a
ROS installation's message headers together: both define the same `std_msgs::Header`
types and `ros::serialization` templates.

Up to release 0.2.0 the headers had a `hypha_msgs/` prefix
(`<hypha_msgs/sensor_msgs/Imu.h>`). It is gone with no forwarding header: an existing
application drops `hypha_msgs/` from each message include line
(`<hypha_msgs/hypha_msgs/NodeState.h>` becomes `<hypha_msgs/NodeState.h>`) and removes
any include directory it added to reach `<package>/<Name>.h` without the prefix.

An application generates its own `.msg` and
`.srv` files with one `hypha_generate_messages()` call per package, an INTERFACE library
of the package name; link it, and the include path and build order follow
(`examples/mrs/CMakeLists.txt`):

```cmake
find_package(hypha CONFIG REQUIRED)
hypha_generate_messages(PACKAGE app_msgs MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg"
                        DEPENDS std_msgs)
target_link_libraries(app PRIVATE hypha::sdk app_msgs)
```

`DEPENDS` names a standard package or one an earlier call declared; never copy or
regenerate a standard package. Every message `M` carries the pointer typedefs of both
ROS generations: `M::Ptr` and `M::ConstPtr` (ROS 1), `M::SharedPtr`, `M::ConstSharedPtr`
and `M::UniquePtr` (ROS 2), and `M_<Allocator>` names `M` as ROS 2's allocator template does. Each field `f` names its type `M::_f_type`, as both
generations' C++ does, and has ROS 2's fluent setter `M& set__f(const M::_f_type&)`. Messages compare with `==`
field by field, as ROS 2's do, `time` and `duration` fields included. Types, constants and the wire format follow ROS 1;
engineering-traps.md gives the rules that surprise (`byte` is signed, a constant out of
its type's range fails generation, no big-endian target).

Code written for ROS 2 includes `<sensor_msgs/msg/point_cloud2.hpp>` and names
`sensor_msgs::msg::PointCloud2`. An application that wants those names generates them for
the packages it uses, its own and standard ones, with one call; hypha-cxx installs none:

```cmake
hypha_generate_ros2_headers(app_ros2_headers PACKAGES std_msgs sensor_msgs app_msgs)
target_link_libraries(app PRIVATE hypha::sdk app_msgs app_ros2_headers)
```

Each message gets `<package>/msg/<name>.hpp` and each service `<package>/srv/<name>.hpp`,
`<name>` spelled as rosidl spells it (`Vector3Stamped` is `vector3_stamped.hpp`). The
header includes `<package/Name.h>` and declares `package::msg` (for a service also
`package::srv`) as a name of `package`, for its own package and every package its fields
reach, as a ROS 2 header makes the types it reaches visible; for a service it adds
`Name_Request` and `Name_Response`. A type that reaches a `time` or
`duration` field also names its field types `builtin_interfaces::msg::Time` and `Duration`,
as in ROS 2. The type is the ROS 1 type: its fields keep their ROS 1 names and types
(`header.seq`, and `stamp.secs`, not ROS 2's `stamp.sec`), so code that reads a ROS 2 field
name fails to compile rather than read another value.
`test/package_consumer/ros2_style.cpp` compiles against these names alone.

---

## 2. Node lifecycle

An application derives from `hypha::ApplicationNode` or `hypha::CycleNode` and returns
`hypha::runNode<App>(argc, argv, "name")` from `main` (`examples/speed`; phases and exit
codes in section 5.4). A program with its own `main()` constructs a `hypha::Node`,
creates its endpoints and calls `run()` or `runFor()` (`sensor_fusion/src/probe_main.cpp`).
Reference: `hypha/node.h` (`Node` and its name rules), `hypha/raw.h` (`RunResult`),
`hypha/application_node.h`, `hypha/topic.h` (`~/x`); the run is execution-model.md §4.

- **Node report.** `runNode` publishes `hypha::introspect()` latched on `~/introspection`
  and `hypha::nodeState()` on `~/state` every second (section 9.1). A program with its
  own `main()` gets neither, nor the refusal of unread parameters: it publishes both
  itself and checks `config().unreadKeys()`. The bindings of one topic give one input
  with the worst verdict; an `Arrivals` input without a new sample is still usable, and
  `setIgnored(true)` makes an input ok, so `inputsOk()` and the state agree. Each
  judgement change is logged (section 5.4, "Input supervision"). A publisher of another
  type on the node's own `~/state` or `~/introspection` is refused with `InvalidTopic`
  (`~/state carries hypha_msgs/NodeState, not <type>`); under `runNode` the start exits 70.
- `HYPHA_DOMAIN` separates whole systems on one host (interface-specification.md).

### 2.1 Library version and ABI revision

The root file `abi.revision` holds the revision; each build reads it into
`HYPHA_ABI_REVISION` (`hypha/hypha_ffi.h`), and the library returns its own from
`hypha_abi_revision()`. Before any other ABI call the SDK compares the two exactly. A
mismatch refuses the start: `Node` and `ConfigContext` throw with `InvalidState`,
`runNode` returns 78 before it reads the parameter file, the Rust crate refuses
`BusNode`, and standard error (not the `[log]` sink) gets
`hypha ABI revision mismatch: SDK expects 1, library reports 2`. Calls that cannot fail,
such as the clock, skip the check; a C ABI caller compares the values itself first. The
library file name carries the revision (getting-started.md), so a binary built against
another revision no longer finds its library: rebuild every consumer.

### 2.2 Waking a node from outside the framework

hypha has no payload-free event channel. A wake-up of no topic (an interrupt thread,
another process) is a small message, such as a `std_msgs/UInt8` code, read by a callback
subscriber or a triggered cycle; every publisher notifies. Latch it for late starters.

### 2.3 Cleaning up

Every `Node` of a process joins one session, and the last one destroyed leaves the bus
and destroys every cycle of the session, whatever order the `Node`s were made in (#97);
a default-constructed `Node` joins nothing and does not count (`hypha/node.h`). A
publisher of a session that has left sends nothing: `publish()` returns false with
`lastPublishStatus()` `InvalidState`, as the C ABI's loan and send do once the node was
destroyed.
A start-up error throws from `Node`, `advertise`, `subscribe` or `timer`, naming the port
or topic and status (`hypha: ports.outputs.command failed: NotAvailable`). Bad bus data
is counted, logged at a bounded rate and dropped, never fatal. No exception crosses the
C ABI; `hypha::require()` turns a `std::expected` into one (`hypha/status.h`).

A late subscriber of a latched topic gets its sample from the publisher's
housekeeping, so the publishing process must run its executor
(execution-model.md §3); a process with its own loop runs bounded turns:

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

A timer with `node.run()` serves it too (`examples/sim_twin/src/sim_clock.cpp`). A
schema needs no executor: the publisher writes its schema record when it is created.

### 2.4 Metrics

`hypha/metrics.h` reads the counters of interface-specification.md "Metrics" (no C++
wrapper for `integrity.*`, `loan.*`). On host time a `node.last_sweep_end_nanos` that
stops moving shows that no thread runs the node's `WaitSet`; under simulated time it
follows the replay clock, so on its own it shows nothing.

## 3. Time and clock

Pace periodic work with a cycle (section 5.4) or `Node::timer()`; there is no blocking
wait. `examples/sim_twin` runs cycles and a timer on the `/clock` of its `sim_clock.cpp`.
Reference: `hypha_base/time.h`, `hypha_base/clock.h`, `hypha/message_stamp.h`,
`hypha/cycle.h` (`CycleTime`, `Hz`); simulated time is execution-model.md §11.

- The framework `/clock` subscriber wakes on the notification every `/clock` publisher
  sends. `/clock` going back ends a cycle's or
  timer's run with `ClockRewound`, and `runNode` restarts the process.
- A `Timer` can be stopped and restarted. `cancel()` stops its calls until `reset()`; a
  canceled timer counts no missed period and costs no wake-up. `reset()` puts the next
  call one period after the call. `state()` returns a `TimerStatus`: `canceled`,
  `period`, and `untilDue`, the time to the next call (empty while canceled). All
  three use the timer's clock, framework time under simulated time, else host
  monotonic time. A cancel or reset from a callback also holds back a call of that
  timer still due later in the same turn; from another thread during a run each
  returns `InvalidState`. A callback may create a timer when the node runs on host
  time or on its own simulated clock. It joins before the run next waits; on a
  simulated clock the node does not own, `Node::timer()` throws. Reference:
  `hypha/node.h` (`Timer`, `TimerStatus`).
- In a replay, a plain subscription or timer callback takes no part in the lockstep:
  it runs as its sample arrives, `hypha::now()` is the clock, and a timer fires once
  per turn with its missed count (execution-model.md §11.2, #153). Only cycles are
  held, ordered and reported.
- Seconds as a `double` must be finite: `Duration::fromSeconds`, `Time::fromSeconds`
  and `Duration * double` throw `std::invalid_argument` for NaN or infinite input
  (a compile error in a constant expression), and clamp finite input outside the int64
  nanosecond range. On a path that must not throw, such as a cycle step,
  `Duration::tryFromSeconds` returns an empty `std::optional` instead (#126).
- There is no implicit `Time` to `RosTime` conversion; never stamp with `steadyNow()`,
  and check a time computed from an input once where it enters:

```cpp
status.header.stamp = time.stamp;                      // in a step
status.header.stamp = hypha::toRosTime(hypha::now());  // in a callback
```

## 4. Publishing

Outside a cycle, `node.advertise<M>(port | hypha::Topic{...}, options...)` creates a
`hypha::Publisher<M>` (`examples/sim_twin/src/sim_clock.cpp`; in a cycle an `Output`).
`node` may be `hypha::Node()`, the handle that joins no session, once a named `Node`
exists (section 1); before that `advertise` throws `... InvalidState: no session exists`.
Reference: `hypha/node.h`, `hypha/publisher.h`, `hypha/topic.h`, `hypha/options.h`.

- A fixed-size message is bounded to its encoded size. Any other is growable, unless
  `hypha::maxPayloadDynamic(bytes)` or the policy's field (section 7) names its largest
  message; a larger one is `TooLarge`. A growable publisher reserves its pool at its first
  slot size, 4096 bytes per sample: give small messages the policy's `initialPayload`
  (a hint, not a bound; a larger message still grows the segment) to keep the topic's
  shared memory small (engineering-traps.md). Sizing an image or cloud pool is the
  developer's job (`examples/large_payload`).
- `subscriberCount()` counts subscribers of every node on the machine at that instant.
  It is not delivery; check it first when a message is costly and nobody may listen.
  It returns `std::optional<size_t>`: empty means the count is unavailable (a disabled
  publisher, or a transport that cannot count), `0` means no subscriber now. Skip work only
  for a counted `0`. The ROS 1 mapping is in
  [ros1-migration-guide.md](ros1-migration-guide.md#104-time-status-and-logging).

## 5. Subscribing

To react to each message as it arrives, create a subscriber with a callback,
`node.subscribe<M>(port | hypha::Topic{"/x"}, callback, options...)`, and run the node.
`examples/sensor_fusion/src/probe_main.cpp` does this with two named functors and
`runFor()`. A bare string names the port table `[ports.inputs.<port>]` (section 9).
Options: a `hypha::Qos` (section 7) and, for a port, `hypha::optionalTopic` (section
5.3). The callback is optional. A subscription on a literal topic wakes the run per
sample; a `*` pattern is read every 1 ms (execution-model.md §7-8). A cycle input port
warns about a missing publisher like a subscriber, once per topic: ports that share a
topic share the warning. A literal topic and a port name side by side, and a ROS 1 node
ported to an `ApplicationNode`:
[ros1-migration-guide.md](ros1-migration-guide.md#103-literal-topic-or-port-name).
Reference: `hypha/subscriber.h` (type check, patterns, the no-publisher warning),
`hypha/node.h` (`Node::subscribe`).

### 5.1 Freshness and the latest value

Every frame carries its publisher's host-monotonic send time. Freshness is judged
for cycle inputs only, at capture and below the C ABI, against each port's silence
limit (`silence_ms`, "Input supervision"). One topic can therefore serve ports with
different limits, and a callback subscriber never discards a frame for its age
(execution-model.md §6). `examples/speed/src/speed_monitor.cpp` acts on the verdict.
Transport time is not source time: a value that needs sensor or event time carries
it in its payload, for example in a ROS `Header`. Arrival period and jitter do not
decide whether a value is valid; a tool that needs them measures the deliveries.

A step reads where a sample came from, from its frame header: `port.sentAt()` is the
host-monotonic send time, `port.sequence()` the send sequence its publisher gave it, and
`port.publisherId()` the publisher instance. On a `Latest` port they describe the held
value and are empty unless `has()`; on an `Arrivals` port they take the index of a sample
of `samples()` and throw `std::out_of_range` past the end. The sequence counts the frames
of one publisher and wraps; two publishers of one topic each count their own. The library
has already refused a repeated or older frame and counted a lost one
([communication-integrity.md](communication-integrity.md)), so a step needs no sequence
check of its own. In a `StepHarness` the sequence and the publisher read zero; the send
time reads zero after `set()` and the given time after `deliver()`.

### 5.2 Two ways to read a subscription

- **Callback.** One call per delivered sample, on the thread that runs the node; the
  run drains each queue and calls the callback once per surviving sample, so the
  queue depth bounds the backlog. Use a named functor (`probe_main.cpp`). A slow
  callback is counted and warned about (execution-model.md §5).
- **Cycle.** One frozen capture of every input per step, then the marked outputs.
  Use `hypha::CycleNode` (section 5.4) for a class, or `hypha::Cycle` (section 5.5)
  when ports are built at run time. Missing and stale inputs reach the step as
  verdicts; the application decides. `examples/scan_cycle/inc/scan_cycle/scan_cycle.h` is a
  software PLC of this kind.

`latestSample()` is for tests and tools. Code that acts on the newest value of a
topic at a rate is a cycle with a `Latest<T>` port; there is no timer-polling
pattern. A cycle owns its periodic registration in the node's default `WaitSet`; its
timer starts only when registration commits, and `node.run()` serves it on the
calling thread. `hypha::require(cycle.health())` and
`hypha::require(port.bindingHealth())` read health without capturing anything; the
node introspection (section 9.1) lists the topics, period, modes and limits of every
cycle. Reference: `hypha/cycle_runtime.h` (`Cycle`), `hypha/cycle.h` (`Hz`,
`CycleTime`, the health records).

### 5.3 Options

The options of a port or an endpoint are tags in its declaration; the port table of
the parameter file is the wiring (`topic`, `silence_ms`, `stagnation_ms`, `ignored`).
There is no options struct and no setter. A port that reads configuration in its
member initializer can compute its options, and even its name: the parameter file
is loaded before the application is constructed. `scan_cycle.h` declares an
optional advisory input; `examples/mrs/managers/transform_manager.cpp` uses `latched`
and `unsupervised`, `examples/mrs/managers/mission.cpp` `perStep(64)`.

| Tag                                             | `Latest` | `Arrivals` | `Output` | `advertise` | `subscribe` |
| ----------------------------------------------- | ---------- | ------------ | ---------- | ------------- | ------------- |
| `optionalTopic`                               | port       | port         | ✓         | port          | port          |
|                                                 |            |              |            |               |               |
| `advisory`, `unsupervised`, `limitMax(d)` | ✓         | ✓           |            |               |               |
| `hostClock`                                   | ✓         |              |            |               |               |
| `perStep(n)`                                  |            | ✓           | ✓         |               |               |
| `latched`, `maxPayloadDynamic(b)`           |            |              | ✓         | ✓            |               |
| a `hypha::Qos` value                          | ✓         | ✓           | ✓         | ✓            | ✓            |
| `diagnostic`                                  |            |              | ✓         |               |               |

"port" marks a tag for a port that names a table: a port on a `hypha::Topic{...}`
literal or one that shares another port's topic takes no `optionalTopic`, and a
sharing port no `Qos`. Any other combination does not compile. What each tag does:
`hypha/options.h`. The `topic` of a port table: without `optionalTopic`, `topic = ""` fails with
`InvalidArgument` and a missing key or table with `NotAvailable`; with it both disable
the port. An invalid name (`InvalidTopic`), a number or a bool, and a plain string
entry `name = "/x"` (`InvalidArgument`) fail either way. Each of these is a refused configuration:
`runNode` exits 78. `Node::subscribe` and `Node::advertise` throw `hypha::ConfigError` for it, so a
port table refused in `setup()` exits 78 as it does in a cycle.

Three tags of an input answer three separate questions; none implies another:

| Tag             | Question it answers                        | `silence_ms`                  | Counts for `inputsOk()` and the node state |
| --------------- | ------------------------------------------ | ----------------------------- | ------------------------------------------ |
| `optionalTopic` | May the deployment leave the port unwired? | as without the tag            | a disabled port is not judged              |
| `advisory`      | Is the node healthy without this input?    | optional; supervised if given | no                                         |
| `unsupervised`  | Does the input have an age limit at all?   | refused; never stale          | yes                                        |

`advisory` and `unsupervised` exclude each other (`start()` fails). An input that
counts for health and is not `unsupervised` needs a `silence_ms` ("Input
supervision"). `ignored = true` in the table is the operator's override of what
`advisory` promises: the port starts with its fault overridden.

### 5.4 Cycle node (L3)

A `hypha::CycleNode` is a class: its ports are members and `step()` is a method. Use
it for a fixed set of topics, one rate, and state in one object. The cycle has the
name of the node. `examples/speed/src/speed_monitor.cpp` and
`examples/scan_cycle/inc/scan_cycle/scan_cycle.h` are cycle nodes. `scan_cycle` is the
reference for a cycle node: its [README](../examples/scan_cycle/README.md) states the
contract of each input and has a test at each of three levels (the logic alone, the step
through `StepHarness`, the started cycle on the bus).

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

`runNode<App>(argc, argv, nodeName, constructorArguments...)` constructs
`App(constructorArguments...)` and runs these phases; a failure logs
`node failed in <phase>: <what>`: the ABI check (section 2.1), *configuration
loading*, *application construction* (read and validate `hypha::config()` here; no
bus yet), *node creation*, *setup* (`setup(Node&)`: endpoints, rate or trigger),
*cycle start*, *configuration check* (no key left unread), *node report*
(section 2), *started* (`started(Node&)`), *run*. Then the application is destroyed
and the `Node` leaves the bus. The exit codes (0, 2, 70, 75, 78):
`hypha/application_node.h`; restarts: tools.md (`hypha launch`). A refused
configuration is a `hypha::ConfigError` (a `ParameterError` among them): throw one
from the constructor or `setup()` to exit 78; `failRun()` gives 70. A bad `node.name` or an
invalid `[log]` table in the parameter file is refused when the file loads and gives 78. A bad
`HYPHA_USE_SIM_TIME` or `HYPHA_REPLAY_LOCKSTEP`, a node name given in code, or a name another process
holds fails *node creation* and gives 70. A program with its own `main` returns 75 itself after `ClockRewound` to be
restarted (`replay-flow-control.md` §4.4). Derive an application that owns worker
threads or composes cycles (`examples/sensor_fusion/src/fusion_main.cpp`) from
`ApplicationNode`; one that steps at a rate or on the samples of its triggers from
`CycleNode`.

- The rate has one source: `rate_hz` under `[cycle]`, `setRate()` in `setup()`, or one
  or more triggers, `setTrigger(port)` once per trigger port ("Triggered cycles"). `rate_hz` with `setRate()` fails the start
  (`keep one`), and so does neither (`neither a rate nor a trigger`).
- An input port takes its topic as `{cycle, "name"}`, `{cycle, hypha::Topic{"/x"}}`,
  `{cycle, "name", hypha::Topic{"/x"}}` or `{cycle, ["name",] hypha::sharing(other)}`,
  then its options; `Output` takes a port name only (`hypha/cycle_latest.h`).
- `start()` binds the ports in declaration order. A failure names the cycle, the
  port and the status: `hypha: cycle 'controller_node': input 'topic_speed': NotAvailable`.
- One process has one `CycleNode`; a second step is a composed `hypha::Cycle` (5.5).
- A `Latest` subscriber queues the policy's `maxDepth`, an `Arrivals` its `perStep`
  (section 7). The cycle history is sized below the ABI (`buffer_size + 1` for
  `Latest`, `buffer_size + perStep` for `Arrivals`) and counts samples, not bytes.

#### Sharing a topic

`hypha::sharing(first)` makes a port a second view of another port's samples, with its
own limit keys and cursor, in any cycle (`hypha/cycle_input.h`). In
`examples/mrs/host/include/mrs_host/plugin_host.h` a second subscription shares the first.

#### Queued work, budget and overflow

An `Arrivals` port examines at most `perStep()` samples per step (default 8); a
sample the age rule refuses still uses a slot. Pending samples beyond a share are
the oldest; they are dropped and counted (execution-model.md §6.5). A `CycleNode`
caps all queued ports together with `arrival_budget` (a positive integer, read at
commit) in its `[cycle]` table, beside `rate_hz`:

- Each queued port asks for its pending samples up to its `perStep()`. Asks above the
  budget share it in equal parts among the ports with work; a port that needs less
  frees the rest. A remainder smaller than the port count goes one sample each, from
  a port that rotates every step.
- A `Latest` port, an idle port, the triggers and a `Transforms` port take nothing
  from the budget. Without a budget each port uses only its `perStep()`.
- `rate_hz` and `arrival_budget` are the only keys of `[cycle]`; another key or a
  value out of range fails the commit, and the log names the key and the value.
  Put root-level keys above the first table header. A composed `Cycle` reads none.
- Simulated time steps one deadline at a time and holds a deadline for its upstream
  nodes (execution-model.md §11.1); `hostClock` is explained in `hypha/options.h`.

#### Triggered cycles

`setTrigger(port)` before `start()` adds an `Arrivals` port to the triggers of the
cycle, which then runs one step per sample of any trigger in place of a rate. Call it
once per trigger port; a repeat adds nothing, and the order of the calls has no
meaning. The samples of all triggers step in send-stamp order, a tie in the
construction order of the ports. In a step the port that fired holds its one sample
and every other trigger holds none, so the step tells the port that fired as the one
whose `samples()` is not empty. Each trigger has its own `silence_ms`: `usable()` and
`stale()` of every trigger port judge its newest sample against it in every step. After
the shortest `silence_ms` without a step a fallback step runs with every trigger empty
and each trigger that is past its own limit `stale()`. The rules are in
execution-model.md §6.6. `examples/mrs/managers/estimation_manager.cpp` and
`control_manager.cpp` are triggered cycles. `start()` fails for a rate with a trigger
("a rate and a trigger exclude each other"), a `Latest` trigger ("the trigger must be
an Arrivals port"), a trigger without `silence_ms`, also when `unsupervised` (the error
names the port), and two triggers on one topic through `sharing()`.

In a `StepHarness` test of a triggered step, `deliver()` the sample of the port that
fires, and `deliver(port, {}, sentAt)` each other trigger with the send time of its
newest sample, or keep a delivery from an earlier step, so that its `usable()` is
judged as in the cycle.
A clock driver is a rate cycle that drives `/clock` for a simulator:
`setClockDriver(options)` with `setRate()`, before `start()` (`hypha/cycle_runtime.h`,
`hypha/cycle_clock_driver.h`; `examples/mrs/simulator/mrs_simulator.cpp`). Its
acknowledgement rules, shared with `hypha play`: `replay-flow-control.md` §4.5.

#### Loss counters

Each loss has its own counter (execution-model.md §6.5); do not add them. iceoryx2
counts no queue evictions: a transport drop shows only as a gap in `overrun_total`.

| Counter                                                                   | Reads         | Counts                                                                                              |
| ------------------------------------------------------------------------- | ------------- | --------------------------------------------------------------------------------------------------- |
| `lost()`                                                                | `Arrivals`  | Samples the history overwrote before this port read them.                                           |
| `bindingHealth().overrun_total`                                         | every input   | History losses and, for a queued input, transport sequence gaps. A `Latest` input counts neither. |
| `bindingHealth().age_rejections_total`                                  | every input   | Samples the age rule refused. They were received, not lost.                                         |
| `bindingHealth().work_limit_drops_total`, `.capture.work_limit_drops` | queued inputs | Samples over the step's share, dropped on purpose; `capture.` counts the latest step only.        |

#### Topic states of an input port

Enabled or disabled is configuration (the port's `topic`, section 5.3); counting for
health is code (`advisory`). `examples/scan_cycle/inc/scan_cycle/scan_cycle.h` declares its
setpoint `optionalTopic, advisory` and falls back when the port is switched off.

- A disabled port has no subscriber and never a value. The node introspection lists it
  with an empty topic, and the node state does not judge it.
- A disabled port reports `NeverArrived`, as an enabled port that has received nothing
  yet. Tell the two apart with `enabled()`, after `start()`.
- An output port follows the same rules; a disabled output has no publisher.
- A failed `start()` reads `hypha: cycle 'control': input 'speed': NotAvailable`, or,
  when the library gave a reason, `hypha: cycle '<name>': <StatusName>: <reason>`.

#### Input values

`examples/speed/src/speed_monitor.cpp` reads `usable()`, `verdict()` and `*port` of a
`Latest` input; the headers describe every accessor. Two counters of every input:

- `received()` counts the samples the source accepted, those the age rule rejected or
  the history overwrote included. It wraps.
- `deliveredCount()` counts the usable samples that became the value: one per new value
  of a `Latest`, each usable sample of an `Arrivals`. It wraps. Use it as a message counter.

An output's `send()` serializes the message into one of its `perStep()` loans at once;
the messages go out after the step returns, in `send()` order (execution model §6.1,
failures §6.3). The log, module `hypha`, shows the text of a step's exception once with
the cycle name (no text for one that is not a `std::exception`).

- A step stamp that `RosTime` cannot hold fails the iteration with `InvalidState`; the
  step is not called, and `runNode` returns 70.
- A port's destructor releases its cycle. While the `WaitSet` runs, the destroy is
  refused: the cycle fails at its next tick with `InvalidState` without reaching the
  freed ports, and the node keeps it until the `Node` is destroyed. A cycle that outlives
  its `Node` does nothing when destroyed.

Reference: `<hypha/cycle_latest.h>`, `<hypha/cycle_arrivals.h>`,
`<hypha/cycle_output.h>`, `<hypha/cycle_input.h>`, `<hypha/cycle.h>`.

#### Test a step without a bus

`hypha::testing::StepHarness` (`<hypha/testing.h>`, not in `hypha/hypha.h`) runs the
step of a cycle that was not started: no node and no shared memory; a parameter file only
for `deliver()`, below.
`crates/hypha-cxx/test/test_cycle_doc_examples.cpp` tests the controller of section 5.4
with it; the header gives the setters and the rules. `send()` outside a step throws
`std::logic_error("hypha: output '<port>': send() outside a step")`: a `setup()` that
sends fails start-up, so send an initial message from the first step.

`harness.deliver(port, value, sentAt)` hands a `Latest` port a value sent at `sentAt`. Each later `step(time)` judges it as a capture does: usable while `time.now - sentAt <= silence_ms` of `[ports.inputs.<name>]` (read when `deliver()` is called, so a `Node` or `ConfigContext` must hold the file), stale after; a port without `silence_ms`, without a table, or tagged `unsupervised` never goes stale. `fresh()` is true in the first step that finds the value usable. `lastKnown()` keeps a value that was usable once. A value that is stale in its first judged step is never held, as in a capture. A `sentAt` after the step time is stale with age 0 until that time; a real cycle under simulated time does not admit such a sample at all. `set()`, `setStale()` and `setNeverArrived()` state a verdict directly; a later `set()` or `deliver()` on the same port replaces the earlier state.

`harness.deliver(port, samples, newestSentAt)` does the same for an `Arrivals` port: the samples go to the next step only, and each later step judges the port by `newestSentAt`, so a publisher that stopped gives `Stale` steps. Samples whose newest is already stale in the step that takes them are not delivered.

The harness moves no clock. In a started cycle `hypha::now()` returns `CycleTime::now` while the step runs; in a harness step it returns the process clock. Step code reads the `CycleTime` it is given. A test of code that still calls `hypha::now()` sets simulated time itself: `hypha::enableSimTime(time.now)` once, `hypha::setSimTime(time.now)` before each `step()`, `hypha::disableSimTime()` at the end. `steadyNow()` is real elapsed time and is never set.

`hypha::testing::McapReplay` (`<hypha/testing/mcap_replay.h>`) replays a recorded MCAP file of ROS 1 messages into a harness, so a golden-sample test is one gtest: `feed(port)` takes the topic each `Latest` or `Arrivals` input resolves (a table topic needs a `Node` that holds the parameter file), `next(harness)` delivers the samples of one step and returns its `CycleTime` for `step()`, while `stepDue()`, and `read<M>(topic)` returns every message of a topic, decoded, with its log time and publish time, for the golden outputs. The steps lie on the grid of the period, as in a replay cycle (execution-model.md §11.1): the first is the first multiple of the period after the earliest fed sample, and each step gets the samples whose MCAP log time, the stamp `hypha play` sends, is strictly before its deadline. Both port kinds get their samples through `deliver()`, so each step judges a port by its newest sample against `silence_ms`. `feedIfPresent(port)` is `feed()` for an optional input that only some recordings hold: it returns `false` and leaves the port unfed when the file has no such topic, and checks a channel that is there as `feed()` does. `endAt(time)` adds the silent tail: the steps go on after the last fed sample through the last deadline at or before `time` (for the end of a golden topic, `read<M>(topic).back().logTime`), so a test sees the ports go stale and the outputs follow. Link `hypha::testing_mcap` (option `HYPHA_BUILD_TESTING_MCAP`, on by default) beside `hypha::sdk`; a refusal throws and names the fault, the file and the topic. `crates/hypha-cxx/test/test_mcap_replay.cpp` tests it against a started cycle on the bus.

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

Give each input that counts for health a silence limit in its table. For an input whose
data reaches hypha through a relay (a device driver, the ROS bridge, a gateway), declare
its life sign in code: `arbiter.setAlive(&VehMode::Life_Cnt_Arbiter)` before `start()`.
An input from a hypha cycle needs none, and a payload that does not change is not
stagnant ([communication-integrity.md](communication-integrity.md)).
`examples/sensor_fusion/config/fusion.toml` sets `silence_ms` and `ignored` per sensor;
the mrs managers (`examples/mrs/managers/`) tag inputs with no age limit `unsupervised`.

```toml
[ports.inputs.arbiter]
topic = "/vehicle/mode"
silence_ms = 250
stagnation_ms = 500      # only with setAlive()
ignored = false          # optional; default false
```

The parameter file cannot switch supervision off, and code sets no limit value:

- `silence_ms` is required on an enabled input that counts for health and is not
  `unsupervised`, optional on an `advisory` one, refused on an `unsupervised` one.
  `unsupervised` with `advisory` fails `start()`. `stagnation_ms` is required with
  `setAlive()` and refused without it.
- A missing required key fails `start()` with `NotAvailable`, and a `CycleNode` under
  `runNode` exits 78 (a composed cycle exits as the application decides):
  `hypha: cycle '<cycle>': input '<port>': missing key 'silence_ms': a required input needs a limit, or the tag hypha::unsupervised: NotAvailable`.
- A limit is 1 ms to 3600 s; `limitMax(d)` lowers the top for the port. A value outside
  fails `start()`; the library never clamps. The limit is inclusive: a value exactly
  `silence_ms` old is usable.
- An unknown key fails `start()`, naming the port and the key (known: `topic`,
  `ignored`, `silence_ms`, and `stagnation_ms` with `setAlive()`).
- A sharing port has no `topic` key; a port without a table name that counts for health
  needs `unsupervised`, else `start()` fails with `a required input without a table needs the tag hypha::unsupervised`.
- A port with no silence limit and no `setAlive()` is never stale. A disabled port needs
  no limits. `ignored = true` starts the port with its fault overridden, and fails
  `start()` on a port with no limit.
- The alive value of a `Latest` port is read from its value, that of an `Arrivals` port
  from the newest sample of the step; a step with no sample keeps the last value.

There is no state enum: `!enabled()`, `received() > 0`, `usable()`, `stale()`,
`stagnant()`, `ignored()` and the cycle's `inputsOk()` answer the questions.
`bindingHealth()` returns the counts with the supervision report: `.stale` and
`.stagnant` are the faults supervision found, also while `ignored` overrides them, as
the input's row on `~/state` shows them; `stale()` and `stagnant()` hide an ignored fault.

Each change of an input's judgement is logged once; execution model §6.4 lists the lines.

Reference: `<hypha/cycle_input.h>` (`stale()`, `stagnant()`, `setAlive()`, `setIgnored()`),
`<hypha/cycle_runtime.h>` (`inputsOk()`).

### 5.5 Cycle composition (L2)

Use `hypha::Cycle` when ports are built at run time, when one state object serves two
rates, or when the ports are not members of a class. The step is a named functor.
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

A composed cycle reads no `[cycle]` table. Declare it before its ports, so each port
is destroyed first and releases the cycle. The rules of §5.4 apply to its ports,
triggers and supervision. Reference: `<hypha/cycle_runtime.h>`.

## 6. Raw and type-erased API

A recorder, a player, a bridge or a benchmark cannot name its message types at compile
time, so it sends a topic identity and bytes. `examples/large_payload/src/large_source.cpp`
publishes loans of a `rawPublisher`, `examples/large_payload/src/large_sink.cpp` reads
them with `subscribeRawOfType`, and `examples/bench/src/bench.cpp` uses `subscribeRaw`.
A `RawType` needs a data type, and its md5 is 32 lower-case hexadecimal digits. A raw
endpoint without a `hypha::Qos` adopts the topic's service (section 7). A raw callback's
bytes are valid only during the callback; copy them to keep them. A wildcard
subscription is read every 1 ms (execution model §8). Reference: `<hypha/raw_api.h>`,
`<hypha/raw.h>`, `LoanedBuffer` in `<hypha/backend/raw_publisher.h>`.

## 7. Service limits and endpoint queues

A topic's service limits (publishers, subscribers, deepest queue) size its shared
memory, so they belong to the topic. A **policy** names them; the application keeps its
policies in one header that both ends of every topic include, as
`examples/large_payload/inc/large_frame_qos.h` does:

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

- Every typed endpoint declares a policy: the `hypha::Qos` option of the port, else
  `QosOf<M>::value`. hypha maps three types itself: `tf2_msgs/TFMessage` to `tfQos`,
  and the node's own report types `NodeState` (`~/state`) and `NodeIntrospection`
  (`~/introspection`) to `reportQos` (2 publishers, 2 subscribers, depth 1, 1 KiB
  first slot: only tools read them, from the observer slots). An application cannot
  remap them.
- A later endpoint with another policy is refused with `QosMismatch`, and the log names
  both policies: `advertise` and `subscribe` throw, a cycle fails `start()`.
- A raw endpoint, a player or a bridge that opens a topic first creates it with
  `default`, and a typed endpoint with another policy is then refused. Start the
  application nodes first, or map the type to `default`.
- Under simulated time a sample a queue drops fails the run (`SequenceGap`, "replay
  not faithful"): a `Latest` queue (`maxDepth`) must hold what the topic sends between
  two steps, an `Arrivals` queue (`perStep()`) one step; else use a deeper policy.
- Ports that share a topic read one subscriber with the deepest `perStep` among them; a
  port with a smaller `perStep` drops the older samples of a step as work-limit drops.
- A latched sample reaches only the subscriber that joined, so a join adds no
  `integrity.sequence_gap` rejection at a connected subscriber.
- `sub.missingSequences()` counts the frames a subscriber lost before it read them (a
  queue overflow adds to it; a loss before a publisher's first accepted frame is not
  counted). `sub.metrics()` returns a `hypha::SubscriberMetrics`: `sequenceGap` counts
  the frames that arrived and were refused because their sequence was not newer, so a
  lost frame raises `missingSequences()` and not `sequenceGap`; `invalidFrame`,
  `corruptPayload`, `schemaMismatch` and `invalidTimestamp` count the other refusals;
  `loanBudget`, `loanHeld` and `loanRefused` describe its owning samples. Both read
  from any thread while the node runs, and give zeros for a disabled subscriber
  (`loanBudget` included). A `RawSubscriber` has the same two; a cycle port has none.

A policy with a deeper queue, such as an IMU at `maxDepth = 200`:
[ros1-migration-guide.md](ros1-migration-guide.md#105-queue-depth).
The pool size and the start-order rules are in [engineering-traps.md](engineering-traps.md);
safe overflow and the frame checks in [interface-specification.md](interface-specification.md#service-limits-and-wildcards).
Reference: `<hypha/qos.h>`, and `<hypha/status.h>` for `Status`, `require()` and `ConfigError`.

## 8. Request and reply

hypha has no RPC. A request and its answer are two topics: the client publishes the
request with an identifier such as a sequence number, and the server publishes the reply
with the same identifier. Latch both when either side can start first. A reply that does
not arrive within the client's own deadline is a timeout the client decides on; it may
send the request again. `examples/mrs/host/include/mrs_host/service.h` carries the MRS
services this way. A `.srv` file still generates `Request` and `Response` message types,
each with its own MD5 (engineering-traps.md).

## 9. Parameters

A node reads its one parameter file (the program argument of `runNode`, or the second
argument of `Node`) through `hypha::config()`. `examples/scan_cycle/inc/scan_cycle/scan_cycle.h`
reads checked scalars, `examples/sensor_fusion/src/sensors_main.cpp` a `[[sensors]]`
list, and `examples/mrs/config/mrs_simulator_teleop.toml` includes another file. The
file is read once, at start: there is no parameter server and no mutation API.

```cpp
const auto settings = hypha::config();                       // config("controller") scopes reads
const double rate = settings.required<double>("rate_hz", hypha::constraints::positive());
const double gain = settings.optional<double>("gain", 1.0);  // the fallback only when absent
for (const auto sensor : settings.list("sensors")) {
    const auto topic = sensor.required<std::string>("topic", hypha::constraints::nonempty());
}
```

- **Reads.** `required<T>()` refuses an absent or malformed value. `optional<T>()`
  gives `std::nullopt` or the fallback only for an absent key: a present value that
  does not read as `T` throws `ParameterError`. The fallback is validated too. Every
  integer type reads by its width and signedness; a value out of range throws.
  `constraints::range(min, max)` is inclusive. Two integer bounds give an integer range
  that compares an integer value exactly (`required<uint8_t>("channels",
  constraints::range(1, 8))`); a floating-point bound gives the floating-point range,
  which also refuses a value that is not finite. Relations between fields are the
  application's to check.
- **Types.** A value keeps its TOML type: `"5"` is no number, `5` no text, `1` no
  `bool`. A whole number reads as a floating-point type. `${NAME}` always gives text.
- **Lists.** An array is a list of scalars or of tables (`[[name]]`), never mixed,
  never nested; an empty array has length zero. `list(name)` throws for a missing or
  scalar name, `at(index)` past the end. A scalar element reads with the key `""`, a
  table element by field, a nested table by a dotted name (`limits.max`). Elements
  read as `bool`, `int64_t`, `double` or `std::string`. A list name may not equal a
  scalar name or be a dotted prefix of one (`InvalidArgument`). Errors name the
  entry: `sensors[2].topic`.
- **Environment.** Any string value, list elements, `[log]` and `include` too, may
  hold `${NAME}`, expanded when its file is read. An unset or empty variable fails
  node creation: ``could not load TOML configuration `<file>` at `<key>`: environment variable `<NAME>` is unset`` (or `is empty`). `$NAME` stays literal. An empty
  variable does not switch off an optional port: its topic is refused.
- **Port tables.** A bare string names `[ports.inputs.<name>]` or
  `[ports.outputs.<name>]`, which hold `topic` (section 5.3); an input and an output
  may share a name. A plain entry, `speed = "/odom_speed"`, is `InvalidArgument`.
- **Reserved names.** `node`, `ports` and `log` belong to the framework: `node.name`
  (section 2), the port tables, and `[log]` (section 11), whose keys the parameter
  API does not return. The top-level keys `include` (below) and `unused` are not
  parameters either. Simulated time is `HYPHA_USE_SIM_TIME` only; a `use_sim_time`
  key is an ordinary key.
- **Unread keys.** `unreadKeys()` lists, sorted, the keys under a view that nothing
  read (`node.name` excluded; a list field as `list[index].field`). `runNode` exits
  78 when one remains after `setup()` and cycle start; a custom `main()` checks
  `hypha::config().unreadKeys()` itself. A value code chooses goes into the manifest
  with `hypha::recordConfig(key, value)`.
- **Keys set on purpose and not read (#119).** Two ways keep a key out of
  `unreadKeys()` and the start-up check without deleting it; a typo beside them is
  still refused. Each lands in the manifest (`~/introspection`) as a row with source
  `SOURCE_IGNORED` whose value is the reason, and is logged once at info.
  - Code: `cfg.ignore(keyOrPrefix, reason)` marks a key, a table or a list entry under
    the view as read by a branch the code chose not to take
    (`config().ignore("model_params", "no TensorRT in this build")`). A key the file
    does not set is fine. An empty key or reason throws `ParameterError`.
  - File: a top-level `unused = ["pose_measurement.pose_measure_uncertainty_time"]`
    lists keys or tables a vendored file sets and this node does not read. Every
    file of a composition may list its own. An entry the composed file does not set,
    such as a typo, refuses the file at load, so the list cannot go stale.

  The check is in the C ABI layer (`hypha_config_ignore`, and `hypha_config_unread_keys`
  leaves ignored keys out), so every SDK gets it.
- **Arrays of scalars.** `cfg.requiredArray<T>(key)` returns `std::vector<T>`; `cfg.requiredArray<T, N>(key)` returns `std::array<T, N>` and refuses any other length. `optionalArray<T>` and `optionalArray<T, N>` return a `std::optional` of the same, empty only when the key is absent. T is any type `required<T>` reads (bool, an integer, float, double, std::string), and each element gets the same conversion and range check. A key that is not an array, an element of the wrong type, an array of tables and a wrong length throw `ParameterError` for the `required*` and the `optional*` form alike, and the message names the key and, for an element, its index (`hypha: parameter 'voxel_weights[2]': expected a number, found "x"`; `hypha: parameter 'extrinsic_t': expected 3 elements, found 4`). An empty array is an empty vector, and a length error for N > 0. Reading an array marks the key as read, so `unreadKeys()` does not list it. An array inside an entry of a table list is refused like a nested list.
- **Porting.** A ROS 1 list parameter, with the fixed-size form:
  [ros1-migration-guide.md](ros1-migration-guide.md#108-a-list-of-scalars-in-the-parameter-file).
- **Without a bus.** A `hypha::ConfigContext` serves `hypha::config()` in a test
  (`examples/mrs/test/test_plugin_host.cpp`); `hypha param validate` checks a file.

### Composing files

`include = ["system.toml"]` at the top level merges files in the listed order; it is
not a parameter. A relative path in `include`, in `path(key)` or in a `[log]` sink
resolves against the directory of the file that wrote it. Tables merge key by key.
The root file overrides its includes: its scalar or list replaces the key at any
depth, with one debug line `hypha: <key> from <root file> overrides <included file>`.
Node creation fails with `InvalidArgument`, naming the file and the key, for a file
that cannot be read, an `include` that is not an array of strings, a scalar or list
key set by two included files, a key that changes kind between files (root file
included), an include cycle, a chain deeper than 16, or more than 256 included files
read in one load (a file included twice counts twice). One file may appear in two
branches with disjoint keys.

Reference: `hypha/config.h`.

### 9.1 Node introspection

`hypha::introspect()` returns this node as a `hypha_msgs::NodeIntrospection`: its
identity, the configuration rows, and every live endpoint, cycle and cycle input
binding with its effective settings. `hypha::nodeState()` returns the
`hypha_msgs::NodeState` that `runNode` publishes on `~/state`; section 2 gives the rule
that judges `state`. A program with its own `main()` publishes both itself. No example
calls them; `hypha node` and `hypha top` ([tools.md](tools.md)) show them. The fields
are in `crates/hypha_msgs/msg/hypha_msgs/NodeIntrospection.msg`, `NodeState.msg` and
the row messages; the row rules and counters in `hypha/introspection.h`. Its C ABI
(`hypha_node_introspect`, `hypha_node_state`) gives `TooLarge` and the needed length.

## 10. Coordinate frame transforms (TF)

TF is a convention on ordinary topics: `/tf` and `/tf_static` carry
`tf2_msgs/TFMessage`. `hypha::TransformBroadcaster` and
`hypha::StaticTransformBroadcaster` (section 10.2) send on them and refuse a transform
that every buffer would refuse; a publisher or an `Output` of that type sends without
the check, as `examples/mrs/managers/transform_manager.cpp` does, and the buffers
still refuse what is invalid.

- Dynamic transforms go to `/tf` when they change; transforms that share a stamp go
  in one message.
- Static transforms go to a `latched` `/tf_static`. A late subscriber gets only the
  last message, so a static broadcaster keeps every static transform it owns, keyed
  by child frame, and sends the whole set each time (`StaticTransformBroadcaster` does).
- The stamp is the time the transform describes: a listener interpolates between
  stamps. A rotation is a unit quaternion (`w = 1` for none).

### 10.1 Looking transforms up in a cycle

A cycle reads TF with a `hypha::Transforms` port from `<hypha/cycle_transforms.h>`,
which `hypha/hypha.h` does not include. The step looks up; nothing waits.
`examples/mrs/host/include/mrs_lib/transformer.h` wraps the port.

```cpp
hypha::Transforms tf{*this};                               // a member of the CycleNode
const auto found = this->tf.lookupLatest("map", "base_link");  // in step(): base_link in map
```

Buffer, clock steps, losses: [execution-model.md](execution-model.md) §6.7.

### 10.2 Looking transforms up in a plain node, and sending transforms

A node without a cycle reads TF with a `hypha::TransformBuffer` from
`<hypha/transform_buffer.h>`, which `hypha/hypha.h` does not include either. The run of
the node ingests every message of `/tf` and `/tf_static` into the buffer, in host time
and under simulated time alike; the lookups are those of `Transforms` and return the
same `std::expected<geometry_msgs::TransformStamped, hypha::TransformError>`.
`hypha::name(error)` gives the error's name for a log line (`"UnknownFrame"`).

```cpp
// or {node, {.tf = "/tf", .tfStatic = "/tf_static"}, hypha::Duration::fromSeconds(10)}
hypha::TransformBuffer tf{node};
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

- `lookup()` and `lookupLatest()` are `const`, never wait, and may be called from any
  thread at any time while the buffer lives, also while `node.run()` runs on another
  thread. Two limits: create and destroy the buffer on the thread that configures the
  node, not while the node runs on another thread; and nothing is ingested unless the
  node runs. The cache (default 10 s) must be positive.
- No lookup waits for a transform to arrive, by design (#91). The buffer fills only
  while the node runs, on the thread that runs it, so a callback that waited would
  block every other callback of the node and, under replay, the clock it waits for.
  A transform that is not there yet is an error the caller handles. ROS 2's
  `lookupTransform` with a timeout and `waitForTransform` have no counterpart: hold
  the message and try again in a later callback, once `/tf` has brought the
  transform (what `tf2_ros::MessageFilter` does), or drop it.
- The buffer declares the `tf` policy on both topics, as `Transforms` does: a topic
  that a raw endpoint or a tool created first, with the `default` policy, refuses it
  with `QosMismatch`, and the constructor throws `ConfigError`.
- A `/tf_static` sample latched before the buffer existed is found once the node has
  run. A refused destroy (the node runs on another thread) is logged, not thrown, and
  the buffer stays attached until the node is destroyed.
- **A refused transform (#123).** A lookup that fails on a frame whose newest
  transform the buffer refused (not finite, not a unit quaternion, a stamp that is no
  time, a second parent) returns `TransformError::Refused` in place of `UnknownFrame`,
  `NotConnected` or `AfterBuffer`: the producer is broken, not late. The frame is the
  target, the source or a frame above either; a later transform of that frame that the
  buffer keeps clears it. The buffer's error log line, at most once a second, names the
  reason, and the node metrics `tf.refused` and `tf.undecodable` count every refused
  transform and every message that does not decode, for good. `Transforms` answers the
  same.
- **Both broadcasters check what they send.** `send()` checks each transform with the
  check every buffer applies on ingest (`hypha_transform_check`, below the C ABI): an
  empty frame name, a stamp that is no time (`nsecs` of a second or more; not read for a
  static transform), a value that is not finite, a rotation that is not a unit
  quaternion. A refused send sends and keeps nothing of its message, returns false,
  `lastPublishStatus()` is `InvalidArgument` and `lastRefusal()` names the child, the
  parent and the reason (`hypha: transform refused: 'base_link' in 'map': ...`). A second
  parent or a cycle needs the other transforms, so only a buffer refuses those. Any
  other false is the publish's, as `Publisher::publish`, with `lastRefusal()` empty.
- `TransformBroadcaster` publishes each `send()` (one `TransformStamped` or a
  `std::vector` as one message) on `/tf` with the `tf` policy; an empty vector sends
  nothing and returns true. `StaticTransformBroadcaster` keeps every transform sent
  through it, by child frame (a later one replaces the earlier in place), and publishes
  one latched message with the whole set on each `send()`. Each is an endpoint of one
  thread at a time. Several static broadcasters may share `/tf_static`; each sends its
  own set.

## 11. Logging

Log with the `HYPHA_LOG_*` macros of `<hypha_log/hypha_log.h>`, one
`HYPHA_LOG_MODULE_INIT("name")` per translation unit, and set levels and files in the
`[log]` table. `examples/speed/src/speed_monitor.cpp` uses the throttled and `_WHEN`
forms; `examples/scan_cycle/config/control.toml` has a rotating file sink. The macro
families and their evaluation rules are in the comment of `hypha_log/hypha_log.h`.

The engine is in `libhypha_ffi`; it is synchronous and has no thread and no buffer,
so a slow disk delays the thread that logs and nothing needs cleaning up. Rust code
logs through the `log` crate, which node creation routes to the same engine; iceoryx2
lines arrive at `Warn` and above under targets such as `iceoryx2/service`. The
counters are the `log.*` metrics (section 2.4).

```toml
[log]
level = "info"                 # modules without an entry; default "debug"
console = true                 # standard error; default true
console_level = "info"         # default "trace"
[log.sink.main]                # any number of named file sinks
path = "log/scan_cycle.log"    # required; relative to this file
level = "debug"                # default "info"
rotation_mb = 5                # 1 to 1024 MiB, default 5
rotation_count = 3             # 1 to 100 kept files, default 3
enabled = true                 # default true
[log.module."localisation"]
level = "debug"                # default log.level; enabled = false is "off"
```

Levels are `trace`, `debug`, `info`, `warn`, `error`, `fatal`, `off`. A line reaches
a target at or above both its module level and the target level. The parameter file
is the only configuration (no environment variable); node creation applies it before
the transport starts, and an invalid table fails creation with `InvalidArgument`.
One process has one logging configuration, set by the first table
([engineering-traps.md](engineering-traps.md)).

A line reads `[2026-10-06T12:00:05.123Z] [sim 412.250000] [WARN] [scan_cycle] text`:
UTC wall time, then, under simulated time only, framework time (in a cycle step the
step's own time). A file rolls to `<path>.1` and on after the line that passed
`rotation_mb`. Logging never stops a node; on Linux the console never blocks a step
and counts the lines nobody read. Validation, rotation and
console rules: [interface-specification.md](interface-specification.md#logging-engine).

## 12. Patterns for event-driven sensor pipelines

A sensor pipeline is a chain of processes; each stage is a callback subscriber that
publishes (section 5.2) or a triggered cycle (section 5.4). `examples/large_payload`
sends megabyte frames on raw loans; `examples/bench` runs a point cloud, an image and
a 1 kHz control topic in one process.

### 12.1 Every publisher notifies; the reader decides

A sample is sent before its wake. A wake carries no sample and no count, several
sends may coalesce, and the reader drains its queue. Which readers wake:
[execution-model.md](execution-model.md) §7. A latched topic wakes a late joiner with
the retained sample; a join with no retained sample wakes nobody.

### 12.2 Callbacks and thread ownership

The thread in `run()` runs every callback, once per queued sample. A publish in a
callback wakes the next process; it never runs downstream code on its own stack. One
endpoint belongs to one thread at a time. Destroy the `Node` after `run()` returns.
A worker thread that publishes, and `Timer::missedCount()`:
[ros1-migration-guide.md](ros1-migration-guide.md#106-threads).

### 12.3 Failure and ordering contract

- One subscription's surviving samples arrive in FIFO order; order across topics or
  processes is not defined, so correlated data carries its acquisition stamp.
- A failed notification after a sent sample is a diagnostic degradation, not a failed
  publish: a retry would duplicate a visible sample. It is logged and counted
  (`transport.failed_notifications`); the sample waits for the next notification of
  its topic.
- An undecodable cycle input sample ([features.md](features.md)) logs `hypha: cycle '<cycle>' input '<port>' (<topic>): a sample did not decode as <type>; dropped (<n> so far)`; a `Latest` keeps its earlier value, an `Arrivals` delivers the others.
- A shutdown or an endpoint change is reported before queued data, so a busy topic
  cannot starve a stop. Each subscriber has its own bounded queue: a stalled consumer
  paces nobody, and a loss the transport cannot count is reported as not available.

### 12.4 Shared-memory lifetime

A raw callback's byte view lives until the callback returns; a typed subscriber
decodes into an owning message. A wake does not synchronize streams: a deskew stage
keeps its own bounded IMU history and, without coverage, sends an explicit rejected
output, never an uncorrected scan.

### 12.5 Sizing and validation

Depth one fits only when the newest frame replaces all older work; otherwise size for
the measured jitter and stay bounded, so overload is counted loss. Validate with
separate processes: publish-to-callback latency at p50 and p99 (the framework's target
is under 500 µs at p99), burst loss, idle CPU, shutdown latency, one stalled consumer.

## 13. Complete API reference

The reference is the doc comments of the public headers; no build runs Doxygen
(`docs/Doxyfile` renders them for a reader who has it). Every result type prints as a
name: `Status::name()`, and `hypha::name()` of a `TransformError`, an `InputVerdict` or
a `StopReason`; a value outside the enum reads `"Unknown"`. `<hypha/hypha.h>` includes
all of them but `hypha/cycle_transforms.h`, `hypha/transform_buffer.h` and `hypha/testing.h`.

| Header                                                                   | Holds                                                                                                                                                                                                |
| ------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `hypha/node.h`                                                         | `Node`, `RunResult`, `StopReason`, `Timer`, `ok`, `nodeName`, `requestStop`                                                                                                            |
| `hypha/application_node.h`                                             | `ApplicationNode`, `runNode`, `failRun`, the exit codes                                                                                                                                        |
| `hypha/callback.h`, `hypha/topic.h`                                  | `bind<&Class::method>(object)`; `Topic`, `validateTopicName`                                                                                                                                   |
| `hypha/options.h`, `hypha/qos.h`                                     | The option tags; `Qos`, `QosOf`, `defaultQos`, `tfQos`                                                                                                                                       |
| `hypha/publisher.h`, `hypha/subscriber.h`                            | `Publisher<M>`, `Subscriber<M>`, `MessagePtr<M>`                                                                                                                                               |
| `hypha/cycle.h`                                                        | `Hz`, `CycleTime`, `InputVerdict`, `CycleHealth`, `BindingHealth`                                                                                                                          |
| `hypha/cycle_node.h`                                                   | `CycleNode`; it includes `cycle_runtime.h` (`Cycle`), `cycle_latest.h`, `cycle_arrivals.h` (with `cycle_arrivals_merge.h`: `visitArrivalsInOrder`), `cycle_output.h`, `cycle_input.h` (with `cycle_sharing.h`: `sharing`), `cycle_supervision.h`, `cycle_port.h` |
| `hypha/cycle_transforms.h`, `hypha/transform_buffer.h`, `hypha/transform_types.h`, `hypha/testing.h` | `Transforms`; `TransformBuffer`, `StaticTransformBroadcaster`; `TransformError`, `TransformTopics`; `testing::StepHarness` |
| `hypha/raw_api.h`, `hypha/raw.h`                                     | Raw endpoints, `lookupDefinition`, `listNodes`; `RawSample`, `NodeInfo`, `Metric`, `SubscriberMetrics`                                                                                                          |
| `hypha/status.h`                                                       | `Status`, `ConfigError`, `require`                                                                                                                                                             |
| `hypha/config.h`                                                       | `config()`, `ConfigView`, `ConfigList`, `ConfigContext`, `ParameterError`; it includes `constraints.h` (`constraints`)                                                                                                 |
| `hypha/introspection.h`, `hypha/metrics.h`                           | `introspect`, `nodeState`, `recordConfig`; `metrics`, `metric`                                                                                                                             |
| `hypha_base/time.h`, `hypha_base/clock.h`, `hypha/message_stamp.h` | `Duration`, `Time`, literals; `now`, `steadyNow`, simulated time; `toRosTime`, `fromRosTime`                                                                                             |
| `hypha_base/short_namespace.h`                                     | the alias `hy` of `hypha`; `HYPHA_NO_SHORT_NAMESPACE` leaves it out                                                                                                                      |
| `hypha_log/hypha_log.h`                                                | The `HYPHA_LOG_*` macros                                                                                                                                                                           |
| `hypha/hypha_ffi.h`                                                    | The C ABI below them all: [interface-specification.md](interface-specification.md)                                                                                                                    |
