# hypha features

Status: inventory and first triage, 2026-11-08; the C++ API simplification of 2026-11-12
is in sections 3 and 4. The owner decides every keep, merge, delete or fix; this page
records the decisions.

Every public capability costs maintenance and buys nothing until an application uses
it. This page lists every public capability once, with
the question it answers and the code that uses it. `scripts/check-features.sh` (run by
`./check.sh`) fails when a C ABI function or a public C++ header appears in no row.

## 1. Rules

1. **One question, one answer.** A feature answers one user question ("is this input too
   old?", "how do I run code at a rate?"). Two features may not answer the same question.
2. **The one-sentence test.** If the difference between two features cannot be told in
   one sentence that a user will remember, merge them or delete one. `maxAge` beside
   `silence_ms` failed this test and became one limit.
3. **A change that adds public surface states three things** in its description: the
   question it answers, why no row below answers it, and what it removes or replaces.
   "Nothing" needs the owner's explicit approval. A public surface is a C ABI function, a
   public C++ header or type, a table key, a message field or a tool flag.
4. **No second path.** A replacement deletes the old mechanism in the same change; there
   are no compatibility paths (`AGENTS.md`).
5. **Users decide.** A feature that no application, example or tool uses outside tests
   is a candidate for deletion. The low-level C++ APIs are listed in section 3.12;
   they get no new application users.

## 2. Triage

Every area of section 3 is triaged after each large feature, and at least once a
quarter. Each row is scored on four questions; each "no" becomes an item of section 4.

| Criterion | Question |
|---|---|
| Needed | Does an application, the MRS port, an example or a tool use it outside tests? |
| Unique | Does no other row answer the same question? |
| Unambiguous | Does its one-sentence rule hold? |
| Well built | One owning layer, a test of the error path, and docs? |

Usage below counts files outside tests in the first application built on hypha (`app`), the MRS port
(`mrs`), the other examples (`ex`) and the Rust tools (`tools`), on 2026-11-08.

## 3. Inventory

### 3.1 Node and process

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Node | How does a process join the bus? | `hypha::Node`, `node.h`, `hypha_node_create`, `hypha_node_destroy`, `hypha_node_name`, `hypha_node_ok`, `hypha_node_shutdown`, `hypha_node_default_waitset` | app 12, mrs 7, ex 11 |
| Short namespace | How do I write less than `hypha::` in C++? | `hy::` (`hy::Node`, `hy::now()`): the namespace alias `namespace hy = hypha;` in `hypha_base/short_namespace.h`, which `<hypha/hypha.h>` and the `hypha_base/` headers include; `HYPHA_NO_SHORT_NAMESPACE` before the first hypha include leaves it out for a program with a global `hy` of its own | app |
| Application lifecycle | How does a node run construction, setup, start and the run in order? | `hypha::ApplicationNode`, `hypha::runNode` (extra arguments go to the application's constructor), `application_node.h`, `status.h` | app 9, mrs 7, ex 4 |
| Node report | What is this node, and are its inputs healthy? | Every `runNode` application publishes its manifest latched on `~/introspection` and its state on `~/state` once a second. `hypha::nodeState()`, `hypha_node_state` | all applications |
| Undecodable input sample | What happens to a cycle input sample that does not decode? | It is counted (`RequiredInput.undecodable`), logged at most once per 10 s, and judged as never sent; the step and the run go on. `hypha_cycle_image_reject_sample`, `HyphaCycleSampleView.sequence` | all applications |
| Node state counters | How is each cycle and input of a node doing, from outside the process? | `hypha_msgs/NodeState`: `cycles` (`hypha_msgs/CycleState`: `steps`, `missed_periods`, `last_step_nanos`, `max_step_nanos`, `overruns`, `output_drops`, and the step-time distribution `step_p50_nanos`, `step_p90_nanos`, `step_p99_nanos` since the commit and `recent_step_p50_nanos`, `recent_step_p90_nanos`, `recent_step_p99_nanos`, `recent_step_max_nanos` over the last 10 to 20 s: a fixed log-linear histogram of 16 buckets per power of two, so a quantile reads at most 1/16 above the true value and never below it; it is counted in the step path with one index and three increments, allocates nothing, and takes 14 KB per cycle (#232), `log_lines_dropped`, `log_write_failures`, `sim_time`; `hypha_msgs/RequiredInput`: `age_nanos`, `stagnant`, `ignored`, `dropped`, `rejected`, `undecodable`. `hypha top` shows them | all applications |
| Input-to-output delay | How long does a node take from an input sample to the output it causes? | `hypha topic delay <input> <output> [--window <n>]` (#232; the `rostopic delay` of a node, no payload decoded): for each output frame, the time since the newest input frame sent at or before it, by the host monotonic send stamps in the frame headers, as median, p99, max and min of the last `--window` outputs, once a second. A cycle output takes its stamp when the step sends it, so a triggered node shows trigger to output and a rate node the age of its input when it leaves. Both topics on one host. `--by-stamp` (match `header.stamp`) is not built: it needs the payload decoded, which the tool avoids; the step-time distribution of the node alone is in `~/state` | tools (`docs/tools.md`) |
| Stop and failure | How does code end the run? | `hypha::requestStop`, `failRun` (`failRun(ConfigError)` ends it with 78), `hypha_waitset_request_stop`, `hypha_waitset_request_failure` | mrs 3, ex 3 |
| Worker thread stop | How does a worker thread of an application sleep, or keep a rate, and end at the node's stop? | `ApplicationNode::stopToken()` (a `std::stop_token`), `hypha::sleepFor`, `hypha::sleepUntil`, `hypha::Rate`, `worker.h`. Header-only: the stop is the one `runNode` already ends the run with | — |
| Hand-over to the publishing thread | How does data from a thread the node does not own (a vendor SDK's callback) reach the one thread that publishes? | `hypha::Handover<Kinds...>` (`handover.h`): `push` from any thread, `pop`/`popFor` on the worker, a depth per kind, drop-oldest counted in `dropped()` and warned in the log, ends at the node's `stopToken()` after draining. Header-only. A `Node::post` onto the run thread was not chosen: it needs a wake of the `WaitSet` from a foreign thread (runtime and ABI work) and puts the driver's conversion on the run thread. The count in `~/state` is not done (needs a `NodeState` field) | #233 |
| Refusal reason | Why did the library refuse a cycle? | `hypha_last_error` (thread-local, cleared by `hypha_cycle_create` and `hypha_cycle_commit`; also set by a refused `hypha_cycle_set_clock_driver` start); the C++ `start()` error carries it | — |
| Domain | Which whole system on the host is this? | `HYPHA_DOMAIN`; Rust: `hypha_node::sdk::domain` | all |
| ABI revision | Was the SDK built for this library? | `hypha_abi_revision`, `abi.revision` | all |
| Callback helper | How does a callback borrow its object? | `callback.h` | SDK |
| Umbrella header | What does an application include? | `hypha.h` | all |

### 3.2 Callback endpoints

Kept beside cycles (decided 2026-11-08). Rule: **a cycle when the node's work is a step
over its inputs at a rate, or on each sample of one or several inputs (a triggered
cycle, #138, #206); a callback when the node owns worker threads, or in a tool or a shim
of a ROS client library.**

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Typed publisher | How do I send a message outside a cycle? | `Node::advertise<T>()`, `publisher.h`, `topic.h`, `hypha_publish_with_qos`, `hypha_publisher_loan_frame`, `hypha_loan_send`, `hypha_loan_drop`, `hypha_publisher_destroy` | app 2, ex 2 |
| Typed loan | How do I write a camera frame or a lidar cloud straight into the shared-memory slot, once? | `Publisher<M>::loan(head, capacityBytes)` -> `Loan<M>` (`loan.h`: `message()`, `data()`, `Loan::put`, `publish(Loan<M>&&[, usedBytes])`) for messages with a single trailing numeric array (`hypha::message_traits::LoanedField<M>`, 15 standard types). Refusals (`PoolExhausted`, `LoanLimit`, `InvalidArgument`) do not count as lost frames. Below: `LoanedBuffer::shrink`, `RawPublisherHandle::reserve`, `hypha_publisher_reserve_frame`, `hypha_loan_shrink`, `HYPHA_STATUS_LOAN_LIMIT` (`docs/cpp-api.md`) | camera and lidar driver ports (#255) |
| Publisher readiness | Is anyone listening yet? | `Publisher::subscriberCount()`, `Output::hasSubscribers()`, `hypha_publisher_subscriber_count`, `hypha_publisher_min_subscriber_buffer` | app 1, tools |
| Typed subscriber | How do I handle each message as it arrives? | `Node::subscribe<T>()`, `subscriber.h`, `hypha_subscribe_typed`, `hypha_subscriber_destroy`, `hypha_subscriber_topic`, `hypha_subscriber_publisher_count` | ex 1 (application tests only) |
| Topic name overloading | Can two message types, or two versions of one type, share a topic name? | Yes (#209): each MD5 is its own stream with isolated queues, slots, and policy. Subscribers report sibling schemas in `Subscriber::metrics().otherSchemas` and `~/state` (`RequiredInput.other_schemas`, `other_schemas_checked`) and warn when only another schema publishes. Untyped readers follow all streams via `StreamFollower` (`hypha_node::sdk`); tools label/filter by `--type`/`--md5`, ROS 1 bridge bridges the first stream. Uses existing MD5 + `HyphaServicePortInfo.schema_hash` (`docs/tools.md`) | tests; tools (`docs/tools.md`) |
| Wake on a sample | When does the run wake for a sample? | Publishers always notify on send. Callback subscribers and triggered-cycle triggers attach a listener and wake immediately; rate-cycle inputs drain on tick (or every turn under sim time) with no listener. Failed notifications count in `transport.failed_notifications`. Wildcard subscriptions poll every 1 ms (triggers at `min(silence_ms)/10`, ≥1 ms); single-stream subscribers run 1 s housekeeping for sibling schemas (`docs/execution-model.md`) | all nodes |
| Service server | How does a plain node or an `ApplicationNode` answer a request from another node? | `Node::advertiseService<Srv>(port or hypha::Service{...}, handler)` returns a `hypha::ServiceServer` (`service_client.h`); the handler gets a `hypha::Request<Srv>` (`service.h`) and calls `reply()`, `refuse("why")` or `defer()`, which gives a `hypha::Deferred<Srv>` that any thread answers later; a request left unanswered, or a `Deferred` destroyed unanswered, is refused; a handler that throws ends its call `ServerLost` and fails the run. `[ports.servers.<port>] service`; a port without a table is refused with the stanza. One server per name across its versions. C ABI: `hypha_service_server_create`, `hypha_service_server_destroy`, `hypha_request_answer`, `hypha_request_defer`, `hypha_request_destroy` (#132) | camera port, tests |
| Service client | How does a node call another node, and what became of the call? | `Node::serviceClient<Srv>(port or hypha::Service{...}[, hypha::timeout(d)])` gives a `hypha::ServiceClient<Srv>`: `call(request, onResult)` returns `CallStatus` (`Sent`, `Busy`, `TooLarge`), and `onResult` runs once on the run's thread with a `hypha::CallResult<Srv>`: `outcome()` (`CallOutcome`: `Answered`, `Refused`, `NoServer`, `NotSent`, `ServerLost`, `Timeout`), `ok()`, `response()`, `mayHaveRun()`, `safeToRetry()`, `describe()`; `ready()`/`exists()`, `pending()`. A call before its server exists is held until one appears. `[ports.clients.<port>] service`, `timeout_ms`, `outstanding`. With `hypha::blocking`, a `hypha::BlockingServiceClient<Srv>`: `callAndWait()`, `waitForServer()`, refused on the run's thread (FSR-43). C ABI: `hypha_service_client_create`, `hypha_service_client_destroy`, `hypha_client_call`, `hypha_client_status`, `hypha_client_wait_for_server` (#132) | tests; the tools next |
| Service policy | Which limits does a service of my `.srv` type get? | `hypha::ServicePolicy`, `hypha::ServicePolicyOf<Srv>` (default `defaultService`: 8 clients and 3 for tools, 2 calls in flight each, 4 KiB each way, 1 s), `backend/service.h` | tests |
| Standard services | Which `.srv` types does the SDK ship? | `std_srvs/Empty`, `std_srvs/Trigger`, `std_srvs/SetBool` (`<std_srvs/SetBool.h>`), as it ships `std_msgs` | mrs, autoware shim |
| Timer | How do I run a callback at a rate? | `Node::timer`; a callback of the running `WaitSet` may create one on host time or on the node's own simulated clock (`hypha_waitset_attach`, #90) | app 1, mrs 3, ex 3 |
| Timer control | How do I stop, restart or inspect a timer, also from a callback? | `Timer::cancel()`, `Timer::reset()`, `Timer::state()` (`TimerStatus`) over `hypha_waitset_timer_cancel` (calls nothing, costs no wake-up), `hypha_waitset_timer_reset` (next call one period after now on the timer's clock), `hypha_waitset_timer_state` (`HyphaTimerState`: canceled, period, time until the next call); plain timers only, a cycle runs at its rate | Autoware port (#47, #59) |

### 3.3 Cycles

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Cycle node | How do I write a node whose work is a step? | `hypha::CycleNode`, `cycle_node.h`; the cycle has the node's name and its rate is `rate_hz` of `[cycle]` | app 10, mrs 7, ex 3 |
| Composed cycle | How do I add a second step, or a cycle in a node I write myself? | `hypha::Cycle`, `hypha::Port`, `cycle.h`, `cycle_runtime.h`, `cycle_port.h`, `hypha_cycle_create`, `hypha_cycle_commit`, `hypha_cycle_destroy`, `hypha_cycle_set_class_based` | app 2, mrs 2, ex 3 |
| Unit-testing a step | How is a step tested without a bus? | `hypha::testing::StepHarness` (`testing.h`, not in `hypha.h`): `set`, `setStale`, `setNeverArrived`, `setStagnant`, `deliver` / `deliverStatic` (ages `Arrivals`, feeds `Transforms` via `hypha_transform_buffer_create_fed` / `hypha_transform_buffer_feed`, #248), `step`, `sent`. Refuses disabled `optionalTopic` ports; does not advance the process clock | tests |
| Silence in a step test | Is a delivered input too old at this step? | `StepHarness::deliver(port, value, sentAt)`: judged at each `step()` by the port's `silence_ms` via `value_usable_within` (no `Arrivals` variant) | tests |
| Golden-sample step test | How does a recorded MCAP fixture drive a step test, and how are its golden outputs read? | `hypha::testing::McapReplay` (`testing/mcap_replay.h`, link `hypha::testing_mcap`, `hypha_mcap_*`): `feed(port)`, `feedIfPresent(port)`, `stepDue()`, `next(harness)`, `endAt(time)`, `read<M>(topic)` -> `Recorded<M>` | tests |
| Test on the bus | How does a test that uses the bus stay apart from other tests and from a running system? | `hypha_add_test(<target>)` (CMake, beside `hypha_generate_messages()`): the test runs in the domain `t_<pid of ctest>_<n>` under the test root `t_`; `hypha clean --domain t_` removes the test domains nobody uses | ex autoware, mrs, scan_cycle, sensor_pipeline |
| Latest input | What is the newest value of a topic? | `hypha::Latest`, `hypha::Input`, `cycle_latest.h`, `cycle_input.h` (it includes `cycle_sharing.h`, the home of `sharing()`); `*port` reads a usable value or throws `hypha::InputNotUsable`, `lastKnown()` reads an old one on purpose; `sentAt()`, `sequence()`, `publisherId()` read the frame header of the held value | app 6, mrs 8, ex 3 |
| Arrivals input | What arrived since the last step, in order? | `hypha::Arrivals`, `cycle_arrivals.h` (it includes `cycle_arrivals_merge.h`, the home of `visitArrivalsInOrder()`), the `arrival_budget` key of `[cycle]`; `sentAt(i)`, `sequence(i)`, `publisherId(i)` read the frame header of sample `i` | app 7, mrs 6 |
| Ports on one topic | How do several ports read one topic? | `hypha::sharing(port)`, `source.h` (internal), `hypha_input_source_create`, `hypha_input_source_destroy`, `hypha_cycle_bind_input` | app 1, mrs 1 |
| Topic fixed in code | How does an input read a topic that no parameter file names? | `hypha::Topic`, `topic.h` | mrs, tests |
| Output | How does a step send? | `hypha::Output::send(message)`, `cycle_output.h`, `hypha_cycle_bind_output`, `hypha_cycle_emission_reserve`, `hypha_cycle_emission_commit` | app 9, mrs 8, ex 4 |
| Step image | What did this step capture? | `hypha_cycle_image_*` | SDK |
| Port options | How is one port captured or sent? | Tags in the declaration (`optionalTopic`, `advisory`, `unsupervised`, `limitMax`, `hostClock`, `perStep`, `newestOnly`, `latched`, `maxPayloadDynamic`, `diagnostic`, a `hypha::Qos`), `options.h` | all cycle users |
| Newest sample only | How does an input or a trigger take only the newest pending sample, as ROS's `queue_size` 1? | `newestOnly` on an `Arrivals` port, `HYPHA_CYCLE_INPUT_MODE_NEWEST`; the passed samples are counted in `work_limit_drops_total` and, for a required input, `~/state` `dropped`; the topic's policy does not change | twist_mux port |
| Service policy of a type | Which service limits does a topic of my message type get? | `hypha::Qos`, `hypha::QosOf<M>` (default `hypha::defaultQos`; `hypha::tfQos` for `tf2_msgs/TFMessage`; `hypha::imageQos` and `hypha::cloudQos` for a large message, named as a port option, never mapped), a `hypha::Qos` as a port option, `qos.h`; replaces `HYPHA_QOS` and the profiles | tests, ex 1 |
| Size of a large message | What bound do I name for an image or a cloud, and what does its pool reserve? | `hypha::serializedSize(message)` and `serializedSize(head, loanedBytes)` (`message_size.h`), `hypha topic pool`, the start-up line of a publisher; the rule in cpp-api.md §7.1 | camera and lidar driver ports (#241) |
| Foreign clock input | How does a simulated-time step read data stamped on another clock? | `hostClock`, `hypha_cycle_set_host_clock` | tests; mrs swarm on `spike/zenoh-swarm` |
| Second rate in a step | How does a step run work at a slower rate, or a timer it starts and stops (a ROS timer beside the loop)? | `hypha::CycleTimer`, `cycle_timer.h`: `poll(time)` gives the periods of an anchored grid on the step's time (`Tick::periods`, `expected`, `elapsed`, `missed()`), `start()`, `start(time)`, `stop()`, `missedTotal()`; header-only, no C ABI, no `~/state` (#267) | tests; mrs trackers (its `Ticker`) |
| Triggered cycle | How do I step once per sample of one or several inputs? | `Cycle::setTrigger` once per trigger port, `hypha_cycle_set_trigger`; the port that fired is the one whose `samples()` is not empty | mrs 3 |
| Clock driver | How does a simulator drive `/clock` in lockstep? | `Cycle::setClockDriver`, `hypha::ClockDriverOptions` (`cycle_clock_driver.h`), `hypha_cycle_set_clock_driver`, `hypha_cycle_expect_participant` | mrs 1 |
| TF input | Where is frame A in frame B at this step? | `hypha::Transforms`, `cycle_transforms.h`, `hypha_cycle_bind_transforms` | mrs 2 |
| TF in a plain node | Where is frame A in frame B at a message stamp, from any thread? | `hypha::TransformBuffer` (`lookup`, `lookupLatest`), `transform_buffer.h`, `transform_types.h` (`TransformError`, `TransformTopics`, shared with `Transforms`), `hypha_transform_buffer_create`, `hypha_transform_buffer_lookup`, `hypha_transform_buffer_destroy`: ingested by the run of the node's `WaitSet`, no step; a lookup never waits (#91) | none yet |
| Static transform broadcaster | How do I send static transforms so that a late subscriber gets all of them? | `hypha::StaticTransformBroadcaster` (`transform_buffer.h`): a latched `/tf_static` publisher that keeps each transform by child frame and sends the whole set each time; it refuses what `TransformBroadcaster` refuses | ex autoware (tests) |
| Transform broadcaster | How do I send dynamic transforms so that an invalid one fails at my node, not at every reader? | `hypha::TransformBroadcaster` (`transform_buffer.h`, `send`, `lastRefusal`), `hypha_transform_check`, `HyphaSentTransform`: the check every buffer applies, below the C ABI; a refused send sends nothing (#123). A plain `Publisher<TFMessage>` sends unchecked, and the buffers keep their check | ex autoware (the shim's `tf2_ros::TransformBroadcaster`) |
| Refused transforms | Is a missing transform late, or did its producer send an invalid one? | `TransformError::Refused`, `HYPHA_TF_REFUSED`, the node metrics `tf.refused` and `tf.undecodable` (#123) | ex autoware (shim: `tf2::InvalidArgumentException`) |

### 3.4 Input limits and supervision

Rule: **`silence_ms` is the input's age limit, from each sample's send time; past it the
input is stale. `stagnation_ms` with `setAlive()` is for a relayed source only: it catches
a source that stopped while its relay still sends. A payload that does not change is not a
fault.**

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Silence limit | Is this input too old to use? | `silence_ms` (required on every enabled input, advisory or not; `0` is no limit), `stale()`, `unsupervised` (the code's way to run without one), `limitMax`, `hypha_cycle_bind_input_supervision`, `HyphaCycleBindingView.supervision` | app 3, mrs 3, ex 1 |
| Untimed input | Has this input no time limit (a ROS timeout of 0.0)? | `silence_ms = 0` or `unsupervised`, `Input::untimed()`, `IntrospectedInput.silence_nanos = 0` (#238) | ex twist_mux |
| Stagnation | Did the source behind a relay (driver, bridge, gateway) stop while the relay still sends? | `stagnation_ms`, `setAlive()`, `stagnant()`, `limitMax` | app 1 |
| Inputs of a step | May this step trust its inputs? | `Cycle::inputsOk()` | app 1 |
| Ignore override | Should this fault count right now? | `ignored`, `setIgnored()`, `cycle_supervision.h` | app 1 |

### 3.5 Time

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Framework time | What time is it for this node? | `hypha::now()`, `hypha_clock_now_nanos`, `hypha_base/time.h` | app 7, mrs 1, ex 4 |
| Clock valid | Does `hypha::now()` read a time yet, or the placeholder zero of a simulated-time node before its first `/clock` sample? | `hypha::isClockValid()`, `hypha_clock_is_valid` (host time: always true; simulated time: once the clock has a value; the rule is Rust's, `hypha_node::clock_is_valid`), `hypha_base/clock.h` | — |
| Host time | How much real time passed? | `hypha::steadyNow()`, `hypha_clock_steady_now_nanos`, `hypha_base/clock.h` | app 2, ex 1 |
| Step time | When is this step? | `CycleTime` (`now`, `stamp`, `dt`, `missed`); `hypha::now()` returns `now` while the step runs | all cycle users |
| Simulated time | Does this process follow `/clock`? | `HYPHA_USE_SIM_TIME` (`1`/`true`; `follow` is refused), `hypha_clock_*sim_time` | tests, tools, ex autoware |
| ROS stamps | How do I write or read a ROS `Header` stamp? | `hypha::toRosTime`, `hypha::fromRosTime`, `message_stamp.h` | app 9 |
| Seconds without a throw | How do I turn computed seconds into a `Duration` on a path that must not throw? | `Duration::tryFromSeconds` (empty for NaN or infinite seconds; `fromSeconds` throws) | tests; for the Autoware port's cycle steps (#126) |

### 3.6 Configuration

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Parameters | What did the node's TOML file set? | `hypha::config()`, `ConfigView`, `config.h`, `config_list.h`, `config_matrix.h`, `hypha_node_config`, `hypha_config_value`, `hypha_config_list_len`, `hypha_config_list_value`, `hypha_config_table_names` | app 8, mrs 10, ex 7 |
| Scalar arrays | How do I read a list of numbers, flags or names? | `ConfigView::requiredArray<T>()`, `requiredArray<T, N>()`, `optionalArray<T>()`, `optionalArray<T, N>()`: each element is read as `required<T>` reads a value; no new C ABI function | none yet |
| Lists inside list entries | Can an entry of `[[sensors]]` hold an array or a `[[sensors.points]]` list? | `sensor.requiredArray<T>("weights")`, `sensor.list("points")` on an entry of `cfg.list("sensors")`; the list is named by its entry, `sensors[1].weights`, for `hypha_config_list_len`, `hypha_config_list_value`, `hypha_config_value` (no new function; #234) | mrs, nmea |
| One value or a list | The key is one value or an array of them (`topic = "/a"` or `[...]`) | `ConfigView::requiredScalarOrArray<T>()`, `optionalScalarOrArray<T>()`: a `std::vector<T>` either way; C++ over the readers above (#234) | ports of ROS parameters |
| Tables under a scope | Which `[ports.inputs.<name>]` tables are there? | `ConfigView::tables()` (a scope for each table, `key()` its name), `hypha_config_table_names`; reads no key (`abi.revision` 14, #234) | twist_mux, robot_localization |
| Matrix parameter | How do I read a ROS matrix parameter, the flat row-major list? | `ConfigView::requiredMatrix<T>(key, rows, cols)` into `Matrix<T>` in `config_matrix.h`; 0 for rows or cols takes it from the length; C++ over `requiredArray` (#234) | MRS controllers |
| Value checks | Is this value in range, or one of a list? | `hypha::constraints` in `constraints.h`, which `config.h` includes (`range(min, max)` with integer bounds compares an integer exactly, in C++; `oneOf({4800, 9600})` or `oneOf({"fast", "slow"})`, in C++ as well: no number rule of Rust is repeated), `hypha_param_validate_number`, `hypha_param_validate_nonempty` | app 4 |
| File names | Where is the file a parameter names? | `ConfigView::path()`, `hypha_config_resolve_key`: against the directory of the file that wrote the key | app 4 |
| Port tables | Which topic does a port read or write? | `[ports.inputs.<name>]`, `[ports.outputs.<name>]`, `hypha_param_topic`, `hypha_topic_validate_public_name` | all |
| Reserved topic names | Which topic names are the framework's? | `/hypha` and every name under `/hypha/` except `/hypha/intercept/` (the state topics of `hypha intercept`): a publisher or subscriber of one is refused with `InvalidTopic` and the log says the prefix is reserved. The transport keeps its own iceoryx2 services there (`hypha/srv/` for services, `hypha/hk/` for housekeeping), and a topic's bus name only drops the leading `/`, so a topic `/hypha/srv/x` would be the server lock of service `/x`. `hypha_core::port::is_reserved_topic_name` | all |
| Values of one port | Where does a value that belongs to one input (the priority of an input of a multiplexer) go, so that the node need not name every input twice? | A key of the input's `[ports.inputs.<name>]`, read with `config("ports.inputs.<name>")` before the cycle starts (constructor or `setup()`). A cycle input's `start()` accepts a key that the application has read and refuses any other that is not `topic`, `ignored`, `silence_ms` or, with `setAlive()`, `stagnation_ms`; any other table (an output, an input of an `ApplicationNode`) leaves it to the unread-keys check (#220) | tests (the `twist_mux` shape) |
| Unread keys | Did the file set a key nobody reads? | `hypha_config_unread_keys` | all (at start) |
| Keys not read on purpose | How do I keep a key that is set on purpose, but not read in this build or mode, without deleting it? | Code: `ConfigView::ignore(keyOrPrefix, reason)`, `hypha_config_ignore`. File: a top-level `unused = [...]`, whose every entry must be set. Both are manifest rows with `IntrospectedConfig::SOURCE_IGNORED` and the reason (#119) | ex autoware (file); tests (code) |
| Configuration without a node | How does a tool read a file before it joins the bus? | `hypha_config_create`, `hypha_config_destroy`, `hypha_config_record_config` | tools |
| Composition | How do files share values? | `include = [...]` | mrs |

### 3.7 Reporting

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Introspection | What is this node made of? | `hypha::introspect()`, `hypha::recordConfig()`, `introspection.h`, `hypha_node_introspect`, `hypha_node_record_config`, `NodeIntrospection` | node report, app 1 |
| Metrics | How many of X happened? | `hypha::metrics()`, `metrics.h`, `hypha_node_metrics`; Rust: `RawSubscriber::metrics` | app 1 |
| Subscriber counters | What did this subscriber refuse, and what loans does it hold? | `Subscriber::metrics()`, `RawSubscriber::metrics()`, `hypha::SubscriberMetrics` (`raw.h`), `hypha_subscriber_counters` | none yet |
| Cycle health | How is this cycle and each binding doing? | `health()`, `bindingHealth()`, `hypha_cycle_health`, `hypha_cycle_binding` (one `HyphaCycleBindingView` per binding: health, held facts, supervision; `hypha_cycle_image_binding` inside the step) | app 3 |
| Sequence gaps | Which samples did this subscriber miss? | `Subscriber::missingSequences()`, `RawSubscriber::missingSequences()`, `hypha_subscriber_missing_sequences`; Rust: `RawSubscriber::missing_sequences`, and `RawSubscriber::counters()`, a `SubscriberCounters` handle that still reads the count (and the metrics) after the subscriber moved into a `WaitSet` attachment (Rust SDK only, no C ABI) | tools |
| Supervision faults | Is this input stale or stagnant? | `stale()`, `stagnant()`, log lines | app 3 |

### 3.8 Logging

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Log | How do I write a log line? | `HYPHA_LOG_*`, `hypha_log/hypha_log.h`, `[log]`, `hypha_log_write`, `hypha_log_enabled`, `hypha_log_module_register` | app 15, mrs 11, ex 7 |
| Slow callback | Which callback is slow, and how do I set what slow means for this node? | The warning names the topic, `timer <period>` or `cycle '<name>'`, how long it took and the budget (`hypha: callback of topic '/x' took 46 ms, budget 10 ms, 9 warnings suppressed`), at most once a second for each. The budget is the node's `[log] slow_callback_ms` (0 to 3600000; 0 writes no line); without it, a timer or a cycle has its own period and a subscription 10 ms. No C ABI function: the key is read in Rust at node creation, and `node.slow_callbacks` counts the same callbacks (#128) | Autoware port |

### 3.9 Discovery and schema

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Node and service lists | Who is on the bus? | `hypha_list_nodes`, `hypha_list_service_ports` | tools |
| Schema | What is the definition of a type I received? | `hypha_schema_lookup`; Rust: `BusNode::resolve_schema_hash` (no C ABI), `hypha::SchemaIdentityCache` (one cache for the tools and the ROS bridge, replaces their copies), `hypha::is_valid_md5` | tools, ros bridge |
| Topic patterns | Does this topic match a pattern? | `hypha::topic_matches_pattern` (Rust, from `hypha_core`) | tools |

### 3.10 Replay

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Replay flow control | How does a replay wait for its nodes? | `/clock_progress` | tools |
| Lockstep wait report | Which node is a lockstep replay waiting for, and why did it stop? | `hypha play`: a notice after 1 s of waiting, and an error that names every awaited node after `--timeout` without progress (`replay-flow-control.md` §6.1, #148) | tools |
| Restart on a backward seek | How does a node start over when time goes back? | exit 75 and `hypha launch` (`HYPHA_REJOIN_AFTER_NANOS`), `HYPHA_STATUS_CLOCK_REWOUND` | tools, all nodes |
| Open-loop playback | Can a replay run without waiting for its nodes, as `ros2 bag play` does? | `HYPHA_REPLAY_LOCKSTEP=0` (or `false`), read by `hypha play` and every node | none yet (#102) |

### 3.11 Messages and tools

| Feature | Question it answers | Where | Users |
|---|---|---|---|
| Message generation | How does an application get its message types? | `hypha_msggen`, `hypha_generate_messages()`; every message compares with `==`, field by field | app, mrs, ex autoware |
| ROS 2 message names | How does code written for ROS 2 include its messages unchanged? | `hypha_generate_ros2_headers(<target> PACKAGES ...)`, `hypha_msggen --language ros2`: `<pkg>/msg/<name>.hpp` and `pkg::msg::Name` over the ROS 1 types, generated by the application | ex autoware |
| sensor_msgs companion headers | How do I use `PointCloud2Iterator`, the `PointField` conversions, `image_encodings` and `distortion_models` as in ROS? | `<sensor_msgs/point_cloud2_iterator.h>`, `point_field_conversion.h`, `image_encodings.h`, `distortion_models.h`: `ros/common_msgs` 1.13.2 unchanged (`crates/hypha-cxx/upstream/common_msgs`, `vendor.toml`), installed beside the messages; their ROS 2 names `<sensor_msgs/...hpp>` forward to them (#242, #265) | ports of lidar and camera drivers |
| Tools | How do I look at, record, replay, launch or bridge a system? | `hypha <tool>`, also `hy <tool>` and the link `hy<tool>` (`hytopic`, ...): `hypha topic`, `hypha top`, `hypha record`, `hypha play`, `hypha convert`, `hypha launch`, `hypha param`, `hypha node`, `hypha foxglove`, `hypha_ros_bridge`, `hypha intercept`, `hypha clean`, `hypha load` (`docs/tools.md`) | operators |
| Topic list | Which topics does the bus have, and of which type, even before one publishes? | `hypha topic list [-v]` (`docs/tools.md`): one line per stream, `name  [type]`, with the MD5 start when a name has several types; it reads the transport's service listing and returns at once, so an advertised topic with no sample sent is listed; `-v` adds the publisher and subscriber counts (`BusNode::list_topics`, `TopicInfo`) | operators |
| ROS 2 bags | How do I replay a ROS 2 bag on hypha? | `hypha convert <bag.db3> -o <run.mcap>`, then `hypha play` (`docs/tools.md`): CDR into ROS 1 serialization from `.msg` definitions, `--xyzirc` for Autoware's point layout | operators |
| Fault injection | How do I change fields of a topic between a producer and its consumers? | `hypha intercept` (config file + latched state topic), driven from `hypha top`'s Intercept tab (`docs/tools.md`) | operators |
| Stale shared memory | How do I remove the shared memory of a domain nobody uses, after a crash or an upgrade? | `hypha clean --domain <prefix>`, or `hypha clean` for the domain of `HYPHA_DOMAIN` (`docs/tools.md`): it keeps a domain while one of its nodes lives, or while the run whose pid follows the prefix lives, and kills nothing. `./clean-shm.sh [--force]` in the repository wipes every domain of the host | operators, tests, `test.sh` |
| Domain shared memory | How much shared memory has a domain written? | `hypha_node::sdk::clean::domain_shared_memory(domain)`: the allocated blocks of the domain's `/dev/shm` objects on Linux, `None` on other systems, which do not tell it; 0 for a domain without objects, an error for a name that is not a domain (Rust SDK only, no C ABI) | `hypha load` |
| Load measurement | How do I measure latency, loss, missed periods and memory under a defined load, and check them against limits? | `hypha load <workload.toml> --report <file>` (`docs/tools.md`): worker processes from a workload file in a private domain, one JSON report, exit 0 / 1 / 70. It replaces nothing; `examples/bench` stays a one-process measurement of three fixed topics | operators, NFR-03 to NFR-06 |

### 3.12 Low level C++ (decided 2026-11-08)

Every tool is Rust and uses the Rust SDK. The C++ SDK has no explicit `WaitSet`: a node
runs its one default `WaitSet` through `Node::run()`. Raw endpoints stay for examples
that move bytes of any type; a new application use needs the owner's approval.

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Raw endpoints | How does a node send and receive bytes of any type? | `raw.h`, `raw_api.h` | ex 2 |
| Explicit run loop | How does a tool own its run loop? | `hypha_node::sdk::WaitSet` (Rust: `new`, `run`, `run_for`; one handle per node at a time), `hypha_waitset_*` (the C++ SDK runs only the node's default one) | tools (Rust) |
| Subscription callbacks in a run loop | How does a Rust tool react to samples without polling? | `hypha_node::sdk::WaitSet::attach_subscription(subscriber, callback)`, `attach_waking_subscription` (samples end the current run once the turn delivered every queued one, so `run_for` returns at once) and `detach`, over `hypha_node::safe::waitset_attach_subscription`, the safe function behind `hypha_waitset_attach`, which takes a Rust closure. A literal topic wakes the run per sample; a wildcard one is read every 1 ms. A callback `Err` fails the run, and `RunResult::err` carries its message. | `hypha intercept`, `hypha play` |
| Owning receive | How does a tool keep a sample after the callback? | `hypha_node::sdk::RawSubscriber::try_receive`, `ReceivedSample` (Rust SDK only, no C ABI) | tools (Rust) |
| CLI tool node name | How do several copies of one command-line tool run in one domain? | The node name `_hyphacli_<tool>` (`hypha::is_cli_node_name`): any number of processes hold it, `hypha top` does not show it, and any node start reclaims it when it is dead. It has no `~/` names and runs no cycle. It replaces the tools' own names and `hypha top`'s own-name filter. | tools: `hypha top`, `hypha topic`, `hypha node`, `hypha record` without `--sync` |

## 4. Decisions and open items

### Open
| Item | Notes |
|---|---|
| ABI folds for next wave | Pre-commit cycle setters (`hypha_cycle_set_class_based`, `_set_trigger`, `_set_clock_driver`, `_expect_participant`) stay separate. `hypha_node_record_config` and `hypha_config_record_config` can merge once the registry reads config rows directly. |
| Input loss for applications | Applications should not re-derive transport loss from raw `bindingHealth()` counters (`overrun_total`, `work_limit_drops_total`, `age_rejections_total`); part of the reporting revision. |

### Decided (archive)
| Item | Decision |
|---|---|
| Callback endpoints beside cycles | Keep both; see §3.2 rule. |
| Input limits | Always read `silence_ms` / `stagnation_ms` from port tables; `limitMax` bounds them in code; `ignored = true` without a limit is refused. An untimed input is declared, never omitted: `silence_ms = 0` or `unsupervised`; it is never stale, and an untimed trigger runs no fallback (#238). |
| Time & stamps | `toRosTime` throws `std::out_of_range` instead of returning `std::optional`. `fromSeconds` / `Duration * double` throw on NaN/Inf (`tryFromSeconds` is the non-throwing form, #126). `hypha::now()` returns `CycleTime::now` during a step. `Rate` and clock sleeps removed in favor of `Node::timer`. |
| Process & threading | Multiple `Node`s share one session (ref-counted) and one config (#97, #208). Concurrent `WaitSet` access is detected via atomic owner token and returns `InvalidState` (#83). `TransformBuffer` lookups never block (#91). |
| Replay & rewinds | Simulated time going backward ends the run with `ClockRewound` and restarts the process (FSR-37); in-place rewind removed (#130). `HYPHA_REPLAY_LOCKSTEP=0` selects open-loop playback (#102). Plain callbacks do not participate in lockstep (#153). |
| QoS & payload bounds | Topic creator stamps `HyphaQos` (`default`: 8 pub / 16 sub / 32 depth; `/clock*` fixed 32/32/32); undeclared subscribers get 3 observer slots and never create non-framework topics; mismatches fail with `QosMismatch`. Fixed-size types bound to encoded size; dynamic types grow from 4 KiB (`initial_payload` hint, up to 32 growths) unless `max_payload_dynamic` is set. `hypha_cycle` owns per-step `EmissionBudget`. Large messages (#241): one pool rule, `qos::pool_samples` (cpp-api.md §7.1); `serializedSize` and `hypha topic pool` give the numbers; `imageQos` / `cloudQos` are port options, not `QosOf` mappings; a pool of 1 MB or more is logged, one over half of `/dev/shm` warns. |
| C++ API & cycle simplification | Single endpoint constructors (`advertise`, `subscribe`, `timer`, throwing on error), compile-time port option tags (`optionalTopic`, `advisory`, etc.), `sharing(port)`, `StepHarness`, `CycleNode` rate from `[cycle]`, `setTrigger` accumulates multiple triggers (#206), `newestOnly` on an `Arrivals` port (#210: the reader takes the newest pending sample, the others are work-limit drops; the topic is unchanged), `hypha::Status` with `name()` (#100), `hypha::Port` & `hypha::Input<M>` public. Legacy builders, `*Options` structs, `prepare()`, `Switchable`, `overflow_policy`, `hypha_waitset_create`/`_destroy`, and dead C ABI items removed. |
