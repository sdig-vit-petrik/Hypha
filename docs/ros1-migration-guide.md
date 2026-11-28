# Migrating a ROS 1 Node to Hypha

Step-by-step guide to porting a ROS 1 (Noetic) C++ node to Hypha while keeping its core logic, comments, and tests intact. Migrate one node at a time and bridge remaining ROS 1 peers via `hypha_ros_bridge`.

References: [cpp-api.md](cpp-api.md), [execution-model.md](execution-model.md), [examples/scan_cycle](../examples/scan_cycle/README.md) (`CycleNode`), [examples/sensor_pipeline](../examples/sensor_pipeline/README.md) (`ApplicationNode`).

---

## 1. Choose the Target Shape

| ROS 1 Node | Hypha Target |
|---|---|
| Timer-driven compute/publish loop | Rate `hypha::CycleNode` (`[cycle] rate_hz`) |
| Callback-driven compute/publish on 1+ inputs | Triggered `hypha::CycleNode` (`setTrigger(arrivalsPort)`) |
| Owns worker threads or wraps a ROS client shim | `hypha::ApplicationNode` (§10) |
| Dynamic runtime port count | `hypha::Cycle` ([cpp-api.md](cpp-api.md)) |

**Stop and design first** if the node relies on: blocking TF waits (`waitForTransform` / `lookupTransform` with timeout), `dynamic_reconfigure`, `actionlib`, `nodelet` in-process zero-copy, or blocking `ros::ServiceClient::call()` inside a callback (Hypha native `.srv` services are in progress; see [design/native-services.md](design/native-services.md)).

---

## 2. Migration Rules

- **Replace glue, preserve logic:** Keep algorithm files, comments, and formatting untouched. Only change lines that fail to compile against Hypha types (§5).
- **Replace hand-written age checks with `silence_ms`:** Keep source-time math (interpolation, deskewing, latency compensation) as logic; replace `now - stamp > timeout` validity checks with port `silence_ms` and `usable()`/`stale()`.
- **One TOML file:** Move all topic names into `[ports.inputs.<name>]` / `[ports.outputs.<name>]`, tuning values into top-level keys (`hypha::config().required<T>()`), cycle rate into `[cycle] rate_hz`, instance name into `[node] name`, and logging into `[log]`. No hardcoded topic fallbacks.
- **Fail restrictive:** Never invent a safe output for missing/stale inputs — confirm the desired degraded behavior explicitly. Out-of-range parameters or unread TOML keys exit **78** (`hypha::ConfigError` / `ParameterError`).

---

## 3. Step-by-Step Procedure

1. **Inventory:** List every subscription (latest-only vs. per-message work), publication (latched?), timer, parameter, remap/namespace, service, TF use, age check, and test.
2. **Design the cycle:**
   - Timer callback → `CycleNode::step(const CycleTime&)` at `[cycle] rate_hz`.
   - Secondary timers over step state → `hypha::CycleTimer` polled at the top of `step()`.
   - Event-driven inputs (no main timer) → `Arrivals<T>` + `setTrigger(port)` (each trigger requires `silence_ms`; handle fallback steps when all triggers are empty).
   - Latest-value subscriptions → `Latest<T>` (replaces manual store-then-read buffers).
   - Per-message subscriptions → `Arrivals<T>` (iterate `port.samples()` in `step()`).
   - Publications → `Output<T>` (`perStep(n)` if a step sends >1 message).
3. **Write `config/<node>.toml`:** Use absolute (`/odom`) or private (`~/cmd`) topic names (relative names are refused). Put per-input app settings (e.g. multiplexer `priority`) in `[ports.inputs.<name>]` and read them via `config("ports.inputs.<name>")` before `start()`. Mark optional inputs `optionalTopic` (`topic = ""` disables them).
4. **Replace CMake/catkin:**
   ```cmake
   find_package(hypha CONFIG REQUIRED)
   hypha_generate_messages(PACKAGE my_msgs MSG_DIR "${CMAKE_CURRENT_SOURCE_DIR}/msg" DEPENDS std_msgs)
   add_executable(my_node src/my_node.cpp src/logic.cpp)
   target_link_libraries(my_node PRIVATE hypha::sdk my_msgs)
   target_compile_features(my_node PRIVATE cxx_std_23)
   ```
   Do not copy standard packages (`std_msgs`, `geometry_msgs`, `sensor_msgs`, `nav_msgs`, `tf2_msgs`). Set `INSTALL_RPATH` for `libhypha_ffi` as in `examples/scan_cycle`.
5. **Replace node glue:** Derive from `hypha::CycleNode`, replace ROS handles with port members, move `main()` to `return hypha::runNode<MyNode>(argc, argv, "my_node");`.
6. **Port tests:**
   - Pure logic unit tests compile against Hypha message headers with minimal token edits (§5).
   - Cycle step tests use `hypha::testing::StepHarness` (`<hypha/testing.h>`): cover `Usable`, `Stale`, and `NeverArrived` verdicts (`deliver()`, `deliverStatic()` for TF, `ConfigContext` for optional ports).
   - Golden-bag regressions use `hypha::testing::McapReplay` (`<hypha/testing/mcap_replay.h>`, link `hypha::testing_mcap`) after converting ROS 1 bags with `mcap convert`.
7. **Verify on the bus:** Run under `HYPHA_DOMAIN=<test>`, inspect `~/introspection`, `hypha topic list -v`, `~/state` (`WAITING` → `INPUTS_USABLE` → `DEGRADED` when input stops), `hypha topic hz`, negative config checks (expect exit **78**), and `Ctrl-C` clean stop.
8. **Deploy:** Add a `[[units]]` entry in your `hypha launch` TOML with `args = ["/etc/hypha/<node>.toml"]` and `HYPHA_DOMAIN`.

---

## 4. ROS 1 to Hypha Map

| ROS 1 | Hypha |
|---|---|
| `ros::init`, `NodeHandle`, `ros::spin()` | `hypha::runNode<App>(argc, argv, "name")` (`argv[1]` = TOML path) |
| `pnh.param("x", v, d)` / rosparam YAML | `hypha::config().required<T>("x")` (or `optional<T>`) ([cpp-api.md](cpp-api.md#9-parameters)) |
| `<remap>`, `ns=`, `~x` | `[ports.inputs.<name>]` / `[ports.outputs.<name>]` with full `/...` or `~/...` |
| `subscribe` (latest / per-message) | `hypha::Latest<T>` / `hypha::Arrivals<T>` |
| `message_filters` sync | `Arrivals` ports; trigger on the last-published input and match `header.stamp` in `step()` |
| `advertise` + `publish` | `hypha::Output<T>::send(msg)` (cycle) or `Publisher<T>::publish(msg)` (plain) |
| `createTimer` / `ros::TimerEvent` | `[cycle] rate_hz` + `CycleTime` (`dt`, `missed`); secondary timers → `hypha::CycleTimer` |
| `ros::ok()` in worker thread | `stop.stop_requested()` from `this->stopToken()` (§10.6) |
| `ros::Rate` / `Duration::sleep()` in worker | `hypha::Rate(hz, stop)` / `hypha::sleepFor(stop, span)` ([cpp-api.md](cpp-api.md#25-a-worker-thread-ends-at-the-nodes-stop)) |
| `ros::Time::now()` for stamp | `time.stamp` in a step; `hypha::toRosTime(hypha::now())` elsewhere |
| `ROS_INFO` / `THROTTLE` / `ONCE` | `HYPHA_LOG_INFO` (`std::format`) / `_THROTTLE(1.0, ...)` / `_WHEN(cond, false, ...)` |
| `tf2_ros::TransformBroadcaster` / `Static...` | `TransformBroadcaster` / `StaticTransformBroadcaster` (or `Output<TFMessage>` with `latched` on `/tf_static`) |
| `use_sim_time` | `HYPHA_USE_SIM_TIME=1` (add `HYPHA_REPLAY_LOCKSTEP=0` for open-loop playback) |
| `rosbag`, `roslaunch`, `roscore` | `hypha record`/`play`, `hypha launch`, none (peer-to-peer SHM) |
| Multi-version topic (`md5sum mismatch`) | Independent per-MD5 streams; mismatches surface in `SubscriberMetrics::otherSchemas` and `~/state` |

---

## 5. Minimal Message & Logic Edits

Standard headers keep ROS 1 paths (`<sensor_msgs/Imu.h>`), namespaces, field names, and `Ptr`/`ConstPtr`. Do not mix ROS and Hypha message headers in one process.

| ROS 1 | Hypha |
|---|---|
| `boost::array<T, N>` (message field) | `std::array<T, N>` |
| `boost::shared_ptr` / `make_shared` (in glue) | `std::shared_ptr` / `std::make_shared` (`<memory>`) |
| `header.stamp.sec` / `.nsec` / `.toSec()` | `stamp.secs` / `stamp.nsecs` / `hypha::fromRosTime(stamp)` (`std::optional<hypha::Time>`) |
| `ros::Time` / `ros::Duration` | `hypha::Time` / `hypha::Duration` (`.seconds()`, `.nanos()`, `fromSeconds()`) |
| `ros::Time::isSystemTime()` / `waitForValid()` | `!hypha::isSimTime()` / delete (steps wait for first `/clock`) |
| `sensor_msgs` companions (`point_cloud2_iterator.h`, `image_encodings.h`, `distortion_models.h`, `point_field_conversion.h`) | Unchanged (installed with SDK; `.hpp` for ROS 2 style) |
| `tf2/LinearMath` | Vendor headers locally (strip `<ros/macros.h>`); do not copy `tf2/convert.h` |
| Callback-only `std::mutex` | Delete (single-threaded `WaitSet`); keep only for worker-thread shared state |
| `pub.publish(msg)` (plain `Publisher`) | Check `[[nodiscard]] bool` return and log `pub.lastPublishStatus().name()` |

---

## 6. Common Traps

- **Relative topics** (`odom_speed`) and **quoted numbers** (`gain = "0.5"`) fail at startup.
- **`Latest` repeats its value** every step; use `Arrivals` for per-message side effects.
- **Do not keep pointers into `Arrivals::samples()`** past the end of `step()` — copy if needed.
- **Never block in `step()` or callbacks** (no sleeps, device I/O, or blocking waits).
- **Use `CycleTimer`, not `Node::timer`**, for secondary periodic work over a cycle's state.
- **Backward `/clock` restarts the process** (exit **75**); do not write manual rewind-reset code.
- **`replay not faithful` under `hypha play --clock`:** increase `perStep` to cover `rate × period` (peak burst), or increase topic `maxDepth` if a second live publisher shares the topic.
- **Stale shared memory after crashes:** run `hypha clean` before debugging transport errors.

---

## 7. Production-Ready Checklist

- [ ] Core logic untouched (except §5 token edits); comments preserved; existing tests pass.
- [ ] Cycle step tests cover `Usable`, `Stale`, and `NeverArrived` verdicts.
- [ ] No ROS headers, `ros::` symbols, or catkin macros remain.
- [ ] All topics come from `[ports.*]` tables; all tuning parameters are range-checked from TOML; all supervised inputs set `silence_ms`.
- [ ] `main()` calls `hypha::runNode`; bus verification (Step 7) and `hypha launch` unit completed.

---

## 10. Porting an Event-Driven Node (`ApplicationNode`)

Use `hypha::ApplicationNode` only when the node owns worker threads or implements a ROS client shim; otherwise prefer a triggered `CycleNode` (which adds input supervision, atomic step outputs, and deterministic lockstep replay).

### 10.2 The application

Read/validate parameters in the constructor (no bus yet → exit **78** on `ConfigError`/`ParameterError`), create endpoints in `setup(Node&)`, start worker threads or send latched messages in `started(Node&)`, and join threads in the destructor:

```cpp
class Locator final : public hypha::ApplicationNode {
 public:
  Locator() : gain_(hypha::config().required<double>("gain", hypha::constraints::positive())) {}

  void setup(hypha::Node& node) override {
    this->scan_ = node.subscribe<sensor_msgs::PointCloud2>("scan", hypha::bind<&Locator::onScan>(*this));
    this->pose_ = node.advertise<geometry_msgs::PoseStamped>("pose");
    this->timer_ = node.timer(hypha::Duration::fromMillis(100), hypha::bind<&Locator::onTick>(*this));
    this->tf_.emplace(node);
  }

 private:
  void onScan(const hypha::MessagePtr<sensor_msgs::PointCloud2>& scan);
  void onTick();
  double gain_;
  hypha::Subscriber<sensor_msgs::PointCloud2> scan_;
  hypha::Publisher<geometry_msgs::PoseStamped> pose_;
  hypha::Timer timer_;
  std::optional<hypha::TransformBuffer> tf_;
};

int main(int argc, char** argv) { return hypha::runNode<Locator>(argc, argv, "locator"); }
```

**Exit codes (`runNode`):** `0` clean stop, `2` usage, `78` config error (`ConfigError`/`ParameterError` or `failRun(ConfigError)`, never restarted by `hypha launch`), `75` `ClockRewound` (always restarted), `70` runtime failure / `failRun(reason)`.

### 10.3 Literal topic or port name

Pass a bare string `"scan"` to resolve `[ports.inputs.scan]` / `[ports.outputs.pose]`; pass `hypha::Topic{"/tf"}` to hardcode a topic. Store endpoints as class members (locals destroy when `setup()` returns). Add `hypha::optionalTopic` if `topic = ""` should disable the endpoint (`enabled() == false`).

### 10.4 Time, status and logging

- `hypha::fromRosTime(stamp)` -> `std::optional<hypha::Time>`; `hypha::toRosTime(hypha::now())` throws `std::out_of_range` if out of range.
- `hypha::requestStop()` and `failRun(...)` are the only calls safe from worker threads during `run()`.
- `pub.subscriberCount()` returns `std::optional<size_t>` — skip expensive message building only when `count.has_value() && *count == 0U`.

### 10.5 Queue depth

The `default` policy caps subscriber depth at **32**. For deeper queues (e.g. 400 Hz IMU with `maxDepth = 200`, up to 1024), define a shared `hypha::Qos` and specialize `hypha::QosOf<M>` in one header included by both ends ([cpp-api.md](cpp-api.md)). Start typed nodes before raw/tool readers when using non-default policies.

### 10.6 Threads

One endpoint belongs to one thread at a time (either the worker owns its publisher or accesses are serialized). Start workers in `started()`, pass `this->stopToken()` by value, pace loops with `hypha::Rate(hz, stop)` / `hypha::sleepFor(stop, span)` ([cpp-api.md](cpp-api.md#25-a-worker-thread-ends-at-the-nodes-stop)), bound any blocking device read with a timeout, call `failRun(err)` on fatal errors, and join in the destructor. Read `timer_.missedCount()`, `sub_.missingSequences()`, and `sub_.metrics()` for diagnostics.

### 10.7 TF in an application

Create `std::optional<hypha::TransformBuffer>` and `StaticTransformBroadcaster` in `setup(node)`, send initial statics in `started()`, and call thread-safe `tf_->lookup(target, source, stamp)` from callbacks or worker threads (never blocks; join workers before destroying the buffer).

### 10.8 A list of scalars in the parameter file

Use `requiredArray<T>(key)` (`std::vector<T>`), `requiredArray<T, N>(key)` (`std::array<T, N>`, throws `ParameterError` on length mismatch), or `optionalArray*`. Use `config().list(key)` for `[[table]]` lists.

### 10.9 Testing an `ApplicationNode`

1. Unit-test ROS-free logic classes directly.
2. Test startup exit **78** on invalid/missing TOML configs (`examples/sensor_pipeline/startup_exit_code.cmake` with `hypha_add_test`).
3. Run a bus replay test comparing outputs by `header.stamp` (`examples/sensor_pipeline/replay_check.sh`). Note that `ApplicationNode` callbacks do not join replay lockstep ([execution-model.md](execution-model.md)).
