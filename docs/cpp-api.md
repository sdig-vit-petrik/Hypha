# C++23 API Guide

Task guide to the Hypha C++23 SDK. For per-method signatures see the public header comments (§12). Related docs: [execution-model.md](execution-model.md), [engineering-traps.md](engineering-traps.md), [interface-specification.md](interface-specification.md), [ros1-migration-guide.md](ros1-migration-guide.md).

Include `<hypha/hypha.h>` for most nodes. Opt-in headers:
- `<hypha/cycle_transforms.h>` (`hypha::Transforms`)
- `<hypha/transform_buffer.h>` (`hypha::TransformBuffer`, `TransformBroadcaster`, `StaticTransformBroadcaster`)
- `<hypha/testing.h>` (`hypha::testing::StepHarness`), `<hypha/testing/mcap_replay.h>` (`McapReplay`)

`hy` is a namespace alias for `hypha` (`hypha_base/short_namespace.h`); define `HYPHA_NO_SHORT_NAMESPACE` before the first Hypha include if your codebase defines a global `hy`.

---

## 1. Quick Start

### Periodic / triggered node (`hypha::CycleNode`)

Use `CycleNode` when work is a single step over typed inputs (`examples/speed`, `examples/scan_cycle`):

```cpp
#include <hypha/hypha.h>
#include <geometry_msgs/Pose.h>
#include <geometry_msgs/Twist.h>

class Follower final : public hypha::CycleNode {
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

```toml
[cycle]
rate_hz = 50.0

[ports.inputs.pose]
topic = "/pose"
silence_ms = 200

[ports.outputs.command]
topic = "/cmd_vel"
```

### Event-driven node (`hypha::ApplicationNode` / `hypha::Node`)

Use `ApplicationNode` (with `runNode`) for callback-driven nodes or nodes owning worker threads; use standalone `hypha::Node` for short probes/tools (`examples/sensor_fusion/src/probe_main.cpp`):

```cpp
hypha::Node node("my_node");
auto out = node.advertise<geometry_msgs::Pose>(hypha::Topic{"/pose"});
auto in  = node.subscribe<sensor_msgs::LaserScan>(
    hypha::Topic{"/scan"}, hypha::bind<&MyClass::onScan>(*this));
return node.run().ok() ? 0 : 1;
```

`hypha::Topic{"/x"}` fixes a literal topic in code; a bare string `"pose"` resolves `[ports.inputs.pose]` / `[ports.outputs.pose]` from the TOML config (§9). A default-constructed `hypha::Node{}` borrows the process's existing session (throws `InvalidState` if no named `Node` exists yet).

### 1.1 Messages and Code Generation

- Standard ROS 1 packages install at `<package>/<Name>.h` (e.g. `<sensor_msgs/Imu.h>`); Hypha messages live at `<hypha_msgs/...>`. Never mix Hypha message headers with a ROS installation's headers in the same process.
- Standard `sensor_msgs` companions are installed unchanged from `ros/common_msgs` 1.13.2: `point_cloud2_iterator.h`, `point_field_conversion.h`, `image_encodings.h`, `distortion_models.h` (and their ROS 2 `.hpp` forwarders).
- Generate custom `.msg`/`.srv` packages in CMake (never regenerate a standard package):

```cmake
find_package(hypha CONFIG REQUIRED)
hypha_generate_messages(PACKAGE app_msgs MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg" DEPENDS std_msgs)
target_link_libraries(app PRIVATE hypha::sdk app_msgs)

# Optional ROS 2-style header aliases (<pkg>/msg/<snake_name>.hpp, pkg::msg::Name):
hypha_generate_ros2_headers(app_ros2_headers PACKAGES std_msgs sensor_msgs app_msgs)
target_link_libraries(app PRIVATE app_ros2_headers)
```

Generated messages provide ROS 1 and ROS 2 pointer typedefs (`Ptr`, `ConstPtr`, `SharedPtr`, `ConstSharedPtr`, `UniquePtr`), `M_<Allocator>`, `_<field>_type`, fluent `set__<field>()`, and field-by-field `operator==`. Field names and wire layout remain ROS 1 (`header.seq`, `stamp.secs`).

---

## 2. Node Lifecycle

`hypha::runNode<App>(argc, argv, "name", ctorArgs...)` runs:
1. **ABI check** — compares `HYPHA_ABI_REVISION` against `hypha_abi_revision()`. Mismatch exits **78** with a stderr diagnostic.
2. **Config load** — reads the single TOML file (`argv[1]`); bad `node.name` or `[log]` exits **78**.
3. **App construction** — read/validate `hypha::config()` here (no bus yet).
4. **Node creation & `setup(Node&)`** — create endpoints, set rate or triggers.
5. **Cycle start & unread-key check** — binds ports; any unread TOML key exits **78**.
6. **Node report & `started(Node&)`** — publishes latched `~/introspection` and 1 Hz `~/state`.
7. **`run()`** — blocks until `SIGINT`/`SIGTERM`, `requestStop()`, `failRun()`, or `ClockRewound`.

**Exit codes:** `0` clean stop, `2` CLI usage, `70` runtime/startup failure (`failRun(reason)`), `75` simulated clock rewound (supervisor restarts), `78` configuration error (`ConfigError` / `ParameterError`, or `failRun(hypha::ConfigError("..."))` from any thread).

- **Session sharing:** All `Node`s in one process share one session (ref-counted; last destroyed leaves the bus) and one parameter file (loading a second different file throws `InvalidState`).
- **Latched topics:** Late joiners receive latched samples during publisher housekeeping, so publishing processes must drive `node.run()` or `node.runFor(...)`.
- **Metrics:** Read runtime counters via `<hypha/metrics.h>`.
- **External wake-up:** Hypha has no payload-free event channel; publish a small message (e.g. `std_msgs/UInt8`) instead.

### 2.5 A worker thread ends at the node's stop

Pass `this->stopToken()` (`std::stop_token`, triggered on any stop/failure before the app destructor runs) into worker threads and pace them with `<hypha/worker.h>`:

```cpp
void grabLoop(std::stop_token stop) {
  hypha::Rate rate(30.0, stop);
  do {
    grabAndPublish();
  } while (rate.sleep()); // false when node stops; overruns increment rate.missedCount()
}
```

- `hypha::sleepFor(stop, span)` and `hypha::sleepUntil(stop, deadline)` are `[[nodiscard]]` and return `false` on stop.
- `hypha::Rate` keeps a fixed host-monotonic grid (skips whole missed periods without bursting; valid range 1 ns–24 h). Always uses steady host time, even under simulated time; use a cycle or `Node::timer()` to follow `/clock`.
- Thread rules: [ros1-migration-guide.md](ros1-migration-guide.md#106-threads).

---

## 3. Time, Clocks, and Timers

- `hypha::now()` returns framework time (`CycleTime::now` inside a step; `/clock` under `HYPHA_USE_SIM_TIME=1`; host wall time otherwise). `hypha::steadyNow()` is always host monotonic time.
- Convert ROS stamps explicitly: `time.stamp` inside a step, or `hypha::toRosTime(hypha::now())` in a callback (throws `std::out_of_range` if out of ROS range). Never stamp with `steadyNow()`.
- `Duration::fromSeconds`, `Time::fromSeconds`, and `Duration * double` throw `std::invalid_argument` on NaN/Inf; use `Duration::tryFromSeconds` on non-throwing paths.
- `Node::timer(period, cb)` supports `cancel()`, `reset()`, and `state()` (`TimerStatus{canceled, period, untilDue}`). Callbacks may create a timer on host time or on a node-owned simulated clock. Plain timers do not participate in replay lockstep ([execution-model.md](execution-model.md)).

---

## 4. Publishing and Zero-Copy Loans

Create publishers outside a cycle with `node.advertise<M>(portOrTopic, options...)` (inside a cycle, use `hypha::Output<M>`).

- **Payload sizing:** Fixed-size messages are bounded to their encoded size. Dynamic messages are growable (starting at 4 KiB per slot unless `Qos::initialPayload` sets a hint) unless capped by `hypha::maxPayloadDynamic(bytes)` or `Qos::maxPayloadDynamic` (oversized sends return `TooLarge`).
- **Typed zero-copy loans (`<hypha/loan.h>`):** Supported for types with a single trailing numeric array (15 standard types: `Image`, `CompressedImage`, `PointCloud2`, `OccupancyGrid`, `*MultiArray`):

```cpp
auto loan = points.loan(head, maxPoints * sizeof(CloudPoint));
if (!loan) return; // PoolExhausted, LoanLimit, TooLarge (not counted as lost frame)
for (size_t i = 0; i < decoded.size(); ++i) loan->put(i, decoded[i]);
loan->message().width = decoded.size();
(void)points.publish(std::move(*loan), decoded.size() * sizeof(CloudPoint));
```

Loan rules:
- String/array lengths in `head` are fixed at `loan()`; scalar fields and contents can be edited via `loan->message()` until `publish()`.
- `loan->data()` is unaligned (ROS 1 has no padding); write via `std::memcpy` or `loan->put(i, val)`, never via aligned pointer casts.
- Dropping an unsent loan returns its slot without consuming `header.seq`. A `data()` span is invalid after `publish()`.
- `subscriberCount()` returns `std::optional<size_t>` (`std::nullopt` if unavailable, `0` if none connected). Only skip expensive work on a counted `0` ([ros1-migration-guide.md](ros1-migration-guide.md#104-time-status-and-logging)).

---

## 5. Subscribing and Cycles

Create callback subscribers with `node.subscribe<M>(portOrTopic, cb, options...)` ([ros1-migration-guide.md](ros1-migration-guide.md#103-literal-topic-or-port-name)). Literal topics wake immediately on arrival; `*` patterns poll every 1 ms.

### Topic Name Overloading (Multi-Schema Streams)
Each `(topic, MD5)` pair is an independent stream with its own queues, slots, and QoS policy. Endpoints of different MD5s on the same topic never see each other's frames. Subscribers report other live schemas on the same topic via `sub.metrics().otherSchemas` and `~/state` (`RequiredInput.other_schemas`, refreshed within ~1 s) and log a throttled warning when only another schema has publishers.

### Freshness and Frame Metadata
Freshness is judged only for cycle inputs (never callback subscribers), using the publisher's host-monotonic send time against `silence_ms`. Read frame metadata in a step with `port.sentAt()`, `port.sequence()`, and `port.publisherId()` (indexed on `Arrivals`).

### Port Options (Compile-Time Tags)

| Tag | `Latest` | `Arrivals` | `Output` | `advertise` | `subscribe` |
|---|---|---|---|---|---|
| `optionalTopic` | port | port | port | port | port |
| `advisory`, `unsupervised`, `limitMax(d)` | ✓ | ✓ | | | |
| `hostClock` | ✓ | | | | |
| `perStep(n)` | | ✓ | ✓ | | |
| `latched`, `maxPayloadDynamic(b)` | | | ✓ | ✓ | |
| `hypha::Qos` value | ✓ | ✓ | ✓ | ✓ | ✓ |
| `diagnostic` | | | ✓ | | |

- `optionalTopic`: missing table or `topic = ""` disables the port (`enabled() == false`); without it, both exit **78**.
- `advisory`: excluded from `inputsOk()` and `~/state` (`silence_ms` optional).
- `unsupervised`: counts for health, refuses `silence_ms`, never goes stale. Cannot combine with `advisory`.

### Cycles (`CycleNode` and `Cycle`)

- **Rate source:** Set exactly one of `[cycle] rate_hz`, `setRate()`, or one or more `setTrigger(arrivalsPort)`.
- **Sharing a topic:** `hypha::sharing(firstPort)` gives a second port its own cursor and supervision limits over the same subscriber.
- **Queued work:** `Arrivals` processes up to `perStep(n)` samples per step (default 8, ≤ policy `maxDepth`). `Output::send()` serializes into `perStep()` loans and flushes after `step()` returns.
- **Triggered cycles:** `setTrigger(port)` steps once per sample across all trigger ports in `(send_stamp, port_order)` order; the firing port is the one with `!samples().empty()`. Each trigger requires `silence_ms`; a fallback step runs after the shortest `silence_ms` with all triggers empty.
- **Clock drivers:** `setClockDriver(options)` with `setRate()` drives `/clock` in lockstep; start time must be in `[0, 2^32)` s (`replay-flow-control.md` §4.5).
- **Sub-rates in a step (`hypha::CycleTimer`):** `<hypha/cycle_timer.h>` polls a non-drifting grid on `CycleTime::now` (`poll(time)` -> `Tick{periods, expected, elapsed}`, `start()`, `stop()`, `missedTotal()`). Works identically in replay and `StepHarness`.
- **Dynamic composition (`hypha::Cycle`):** Use when ports are created at runtime (store ports in `std::deque` so addresses stay stable; declare `Cycle` before its ports so ports destroy first).

### Input Supervision

```toml
[ports.inputs.arbiter]
topic = "/vehicle/mode"
silence_ms = 250       # required for supervised inputs (1 ms..3600 s, inclusive)
stagnation_ms = 500    # required iff code calls port.setAlive(&Msg::counter)
ignored = false        # optional override (refused if port has no limit)
```

Query with `enabled()`, `received()`, `deliveredCount()`, `usable()`, `stale()`, `stagnant()`, `ignored()`, and `cycle.inputsOk()`. `bindingHealth()` exposes raw faults (including while ignored) and separate loss counters: `lost()`, `overrun_total`, `age_rejections_total`, `work_limit_drops_total`.

### Unit Testing Steps (`StepHarness` & `McapReplay`)

`<hypha/testing.h>` runs a `CycleNode`/`Cycle` without a bus or shared memory:
- `deliver(port, val, sentAt)` (for `Latest`), `deliver(port, vec, newestSentAt)` (for `Arrivals`), `deliver(tfPort, vec)` / `deliverStatic(tfPort, vec)` (for `Transforms`), plus direct verdict setters `set()`, `setStale()`, `setNeverArrived()`, `setStagnant()`.
- `step(node, time)` runs one step and advances `received()`/`deliveredCount()`; inspect outputs with `sent(outPort)`. `send()` outside a step throws `std::logic_error`.
- `StepHarness` does not move the process clock; use `enableSimTime`/`setSimTime`/`disableSimTime` if tested code calls `hypha::now()`.
- `hypha::testing::McapReplay` (`<hypha/testing/mcap_replay.h>`, link `hypha::testing_mcap`) drives golden-sample tests from recorded MCAP files via `feed(port)`, `feedIfPresent(port)`, `endAt(time)`, `stepDue()`, `next(harness)`, and `read<M>(topic)`.

---

## 6. Raw / Type-Erased API

For recorders, bridges, and benchmarks (`<hypha/raw_api.h>`, `<hypha/raw.h>`, `examples/large_payload`):
- `rawPublisher` / `LoanedBuffer` publish raw bytes; `subscribeRawOfType(topic, RawType{name, md5}, cb)` reads one MD5 stream; `subscribeRaw(pattern, cb)` reads all existing streams.
- Callback byte slices are borrowed for the callback duration only; copy if needed.

---

## 7. Service Limits and QoS Policies

Define shared policies in one header included by both publishers and subscribers (`<hypha/qos.h>`):

```cpp
namespace my::policies {
inline constexpr hypha::Qos camera{
    .name = "camera", .publishers = 1, .subscribers = 4, .maxDepth = 2,
    .maxPayloadDynamic = 8'000'000};
}
template <> struct hypha::QosOf<my_msgs::Image> {
  static constexpr hypha::Qos value = my::policies::camera;
};
```

- Built-in policies: `defaultQos` (8 pub / 16 sub / depth 32), `tfQos` (for `TFMessage`), `reportQos` (for `NodeState`/`NodeIntrospection`).
- All endpoints on the same `(topic, MD5)` stream must agree on the policy or creation fails with `QosMismatch`. Start typed nodes before raw/tool readers when using non-default policies.
- Subscriber diagnostics: `sub.missingSequences()` (dropped frames before read) and `sub.metrics()` (`SubscriberMetrics`: `sequenceGap`, `invalidFrame`, `corruptPayload`, `schemaMismatch`, `invalidTimestamp`, `loanBudget`, `loanHeld`, `loanRefused`, `otherSchemas`).
- Deep queue example: [ros1-migration-guide.md](ros1-migration-guide.md#105-queue-depth).

---

## 8. Request and Reply

Native typed `.srv` services are in progress ([design/native-services.md](design/native-services.md), Stage 1a transport landed). Until Stage 1b/1c lands in the C++ SDK, request/reply uses paired topics with a correlation ID (`examples/mrs/host/include/mrs_host/service.h`). `.srv` files already generate `Request` and `Response` types.

---

## 9. Parameters

Read the process's single TOML file via `hypha::config()` (`<hypha/config.h>`). No runtime mutation or parameter server exists.

```cpp
const auto cfg = hypha::config("controller");
const double rate = cfg.required<double>("rate_hz", hypha::constraints::positive());
const double gain = cfg.optional<double>("gain", 1.0);
const auto weights = cfg.requiredArray<double, 3>("weights");
for (const auto s : cfg.list("sensors")) {
  const auto topic = s.required<std::string>("topic", hypha::constraints::nonempty());
}
```

- **Strict types:** No implicit string/int/bool conversions (ints convert to float/double). `optional<T>()` returns empty/fallback only when absent; wrong type throws `ParameterError`. `constraints::range(min, max)` is inclusive (exact for integers; rejects NaN/Inf for floats).
- **Arrays & lists:** `requiredArray<T>` / `requiredArray<T, N>` and `optionalArray*` read scalar arrays ([ros1-migration-guide.md](ros1-migration-guide.md#108-a-list-of-scalars-in-the-parameter-file)). `list("name")` reads `[[name]]` table lists (no nested or mixed arrays).
- **Environment variables:** `${NAME}` expands in any string (including `include` and `[log]`); unset/empty fails load. `$NAME` stays literal.
- **File composition:** Top-level `include = ["base.toml"]` merges tables key-by-key (max depth 16, max 256 files; relative paths resolve against the declaring file). Root overrides includes; conflicting keys across siblings fail load.
- **Port tables:** `[ports.inputs.<name>]` and `[ports.outputs.<name>]` may hold extra app keys if read via `config("ports.inputs.<name>")` before `start()`.
- **Unread & ignored keys:** `runNode` exits **78** if any key remains unread after `setup()` + cycle start. Suppress intentional unused keys via `cfg.ignore(keyOrPrefix, reason)` in code or top-level `unused = [...]` in TOML (both appear in `~/introspection` as `SOURCE_IGNORED`).
- **Tests:** Use `hypha::ConfigContext` without a bus; validate files offline with `hypha param validate`.
- **Introspection:** `hypha::introspect()` returns `NodeIntrospection`; `hypha::nodeState()` returns `NodeState` (`runNode` publishes both automatically).

---

## 10. Coordinate Frame Transforms (TF)

TF uses `/tf` (dynamic) and latched `/tf_static` (`tf2_msgs/TFMessage`, `tfQos`):
- **In a cycle (`<hypha/cycle_transforms.h>`):** `hypha::Transforms tf{*this};` -> `tf.lookup(target, source, time)` / `tf.lookupLatest(target, source)`.
- **In a plain node (`<hypha/transform_buffer.h>`):** `hypha::TransformBuffer tf{node};` (thread-safe `const` lookups; create/destroy on the config thread; ingests while `node.run()` runs).
- **Never blocks:** Lookups never wait for future transforms; handle `std::expected<TransformStamped, TransformError>` directly (`hypha::name(err)`).
- **Validation & broadcasters:** `TransformBroadcaster` (`/tf`) and `StaticTransformBroadcaster` (keeps full set by child frame, republishes latched `/tf_static`) validate via `hypha_transform_check` (non-empty frames, finite translation, unit quaternion, valid stamp) before sending. Invalid transforms received by a buffer yield `TransformError::Refused` and increment `tf.refused`.

---

## 11. Logging

Use `HYPHA_LOG_MODULE_INIT("name")` and `HYPHA_LOG_*` (`<hypha_log/hypha_log.h>`). Logging is synchronous in `libhypha_ffi` (no background thread) and configured once per process via `[log]`:

```toml
[log]
level = "info"                 # trace|debug|info|warn|error|fatal|off
console = true
console_level = "info"
[log.sink.main]
path = "log/node.log"          # relative to this TOML file
level = "debug"
rotation_mb = 5                # 1..1024 MiB
rotation_count = 3             # 1..100
[log.module."localisation"]
level = "debug"
```

---

## 12. Header Reference

`<hypha/hypha.h>` includes all headers below except `cycle_transforms.h`, `transform_buffer.h`, and `testing.h`. Render HTML docs with `doxygen docs/Doxyfile`.

| Header | Contents |
|---|---|
| `hypha/node.h`, `application_node.h`, `worker.h` | `Node`, `RunResult`, `StopReason`, `Timer`, `ApplicationNode`, `runNode`, `failRun`, `stopToken`, `sleepFor`, `sleepUntil`, `Rate` |
| `hypha/publisher.h`, `subscriber.h`, `loan.h` | `Publisher<M>`, `Subscriber<M>`, `Loan<M>`, `MessagePtr<M>` |
| `hypha/cycle_node.h`, `cycle.h`, `cycle_timer.h` | `CycleNode`, `Cycle`, `Latest`, `Arrivals`, `Output`, `sharing`, `CycleTimer`, `Hz`, `CycleTime`, `CycleHealth`, `BindingHealth` |
| `hypha/options.h`, `qos.h`, `callback.h`, `topic.h`, `status.h` | Port option tags, `Qos`, `QosOf`, `bind`, `Topic`, `Status`, `ConfigError`, `require` |
| `hypha/config.h`, `constraints.h` | `config()`, `ConfigView`, `ConfigList`, `ConfigContext`, `ParameterError`, `constraints` |
| `hypha/cycle_transforms.h`, `transform_buffer.h`, `transform_types.h` | `Transforms`, `TransformBuffer`, `TransformBroadcaster`, `StaticTransformBroadcaster`, `TransformError` |
| `hypha/testing.h`, `testing/mcap_replay.h` | `StepHarness`, `McapReplay` |
| `hypha/raw_api.h`, `raw.h`, `introspection.h`, `metrics.h` | Raw endpoints, `SubscriberMetrics`, `introspect()`, `nodeState()`, `metrics()` |
| `hypha_base/time.h`, `clock.h`, `short_namespace.h`, `hypha/message_stamp.h` | `Duration`, `Time`, `now`, `steadyNow`, sim time, `toRosTime`, `fromRosTime`, `hy::` alias |
| `hypha_log/hypha_log.h`, `hypha/hypha_ffi.h` | `HYPHA_LOG_*` macros, generated C ABI header |
