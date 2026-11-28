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
| Node state counters | How is each cycle and input of a node doing, from outside the process? | `hypha_msgs/NodeState`: `cycles` (`hypha_msgs/CycleState`: `steps`, `missed_periods`, `last_step_nanos`, `max_step_nanos`, `overruns`, `output_drops`), `log_lines_dropped`, `log_write_failures`, `sim_time`; `hypha_msgs/RequiredInput`: `age_nanos`, `stagnant`, `ignored`, `dropped`, `rejected`, `undecodable`. `hypha top` shows them | all applications |
| Stop and failure | How does code end the run? | `hypha::requestStop`, `failRun` (`failRun(ConfigError)` ends it with 78), `hypha_waitset_request_stop`, `hypha_waitset_request_failure` | mrs 3, ex 3 |
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
| Typed loan | How do I write a camera frame or a lidar cloud straight into the shared-memory slot, once? | `Publisher<M>::loan(head, capacityBytes)` gives a `Loan<M>` (`loan.h`): `message()` for the small fields, `data()` a `std::span<std::byte>` over the loaned field in the slot, unaligned, written with `std::memcpy` or `Loan::put(index, value)`; `Publisher<M>::publish(Loan<M>&&[, usedBytes])` encodes the small fields around it in one pass, shrinks to `usedBytes`, seals and sends, the bytes those of `publish(const M&)`. The loaned field (`hypha::message_traits::LoanedField<M>`) is the message's single top-level array of numbers with only fixed-size numbers after it: 15 standard types (`Image`, `CompressedImage`, `PointCloud2`, `OccupancyGrid`, the eleven `*MultiArray`). The lengths of the head's strings and arrays are fixed at the loan. A refused loan is no lost frame (`PoolExhausted`, `LoanLimit`); a loan of another publisher or a moved-from loan is `InvalidArgument`. Below it: `LoanedBuffer::shrink`, `RawPublisherHandle::reserve`, `hypha_publisher_reserve_frame`, `hypha_loan_shrink`, `HYPHA_STATUS_LOAN_LIMIT`. A cycle `Output` has no loan yet. | camera and lidar driver ports (#255) |
| Publisher readiness | Is anyone listening yet? | `Publisher::subscriberCount()`, `Output::hasSubscribers()`, `hypha_publisher_subscriber_count`, `hypha_publisher_min_subscriber_buffer` | app 1, tools |
| Typed subscriber | How do I handle each message as it arrives? | `Node::subscribe<T>()`, `subscriber.h`, `hypha_subscribe_typed`, `hypha_subscriber_destroy`, `hypha_subscriber_topic`, `hypha_subscriber_publisher_count` | ex 1 (application tests only) |
| Topic name overloading | Can two message types, or two versions of one type, share a topic name? | Yes (#209, owner 2026-11-27): each definition (MD5, not the type name: two type names of one definition share a stream and its policy) is its own stream of the topic, with its own subscribers, queues, publisher slots and policy, so `subscribe<A>` and `subscribe<B>`, or a `Latest<A>` and an `Arrivals<B>` of one port name, never meet the other's frames and count none of them. The case it is for is a migration: version 1 and version 2 of one type run on one topic while nodes are rebuilt. Another version is not hidden: every subscriber of one stream reports the other definitions of its topic that a live publisher sends, refreshed by its node's housekeeping within a second, in its counters (`Subscriber::metrics().otherSchemas`, empty until checked) and in `~/state` (`RequiredInput.other_schemas`, `other_schemas_checked`). The verdict says what the data says: a subscriber whose version nobody publishes stays `NeverArrived` only if it never had a publisher; once it had one it goes `Stale` after its `silence_ms`, and stays `Usable` on the last value without a limit. While its own stream has no publisher it logs `topic '<t>' has publishers, none of type <T> (<md5>): published are ...` after its no-publisher delay, again on a change, at most once in 10 s. A subscription without an MD5 reads every stream that exists at its creation, woken by each; a reader that must follow a name through a migration watches the graph event and subscribes to each stream by its hash (`StreamFollower` in the Rust SDK, which the tools and the ROS bridge share). The tools follow each stream: `hypha record` writes one MCAP channel for each, `hypha play` publishes each on its own publisher, `hypha topic echo`/`hz`/`bw` label and measure each (`--type`, `--md5` choose), `hypha top` shows a row for each with its type, MD5 and the other versions of an input, and the ROS 1 bridge bridges the first stream of a name (ROS 1 carries one type per topic), saying so once. No API of its own: the MD5 every typed endpoint already carries; `HyphaServicePortInfo.schema_hash` lists the streams | tests; tools (`docs/tools.md`) |
| Wake on a sample | When does the run wake for a sample? | When something reads the topic on arrival. Every publisher notifies after a send (no option, in any SDK or the C ABI). A callback subscription and each trigger of a triggered cycle get a listener when attached, so the run wakes at once; a rate cycle's input is drained at its tick (under simulated time on every turn) and has no listener, so its publishers wake nobody. An idle node does not wake for a queue: nothing polls one, and a failed notification is counted (`transport.failed_notifications`); a node with a subscriber of one stream wakes once a second, and every 1 ms during a refresh round, for the housekeeping that refreshes the other streams of its topics. Each stream of a topic has its own event service, and a subscription without an MD5 on a literal topic listens to each of its streams. A wildcard pattern has no event service: a subscription reads it every 1 ms, and a triggered cycle with a wildcard trigger reads its triggers every tenth of its shortest silence limit, at least every 1 ms. | all nodes |
| Timer | How do I run a callback at a rate? | `Node::timer`; a callback of the running `WaitSet` may create one on host time or on the node's own simulated clock (`hypha_waitset_attach`, #90) | app 1, mrs 3, ex 3 |
| Timer control | How do I stop, restart or inspect a timer, also from a callback? | `Timer::cancel()`, `Timer::reset()`, `Timer::state()` (`TimerStatus`) over `hypha_waitset_timer_cancel` (calls nothing, costs no wake-up), `hypha_waitset_timer_reset` (next call one period after now on the timer's clock), `hypha_waitset_timer_state` (`HyphaTimerState`: canceled, period, time until the next call); plain timers only, a cycle runs at its rate | Autoware port (#47, #59) |

### 3.3 Cycles

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Cycle node | How do I write a node whose work is a step? | `hypha::CycleNode`, `cycle_node.h`; the cycle has the node's name and its rate is `rate_hz` of `[cycle]` | app 10, mrs 7, ex 3 |
| Composed cycle | How do I add a second step, or a cycle in a node I write myself? | `hypha::Cycle`, `hypha::Port`, `cycle.h`, `cycle_runtime.h`, `cycle_port.h`, `hypha_cycle_create`, `hypha_cycle_commit`, `hypha_cycle_destroy`, `hypha_cycle_set_class_based` | app 2, mrs 2, ex 3 |
| Unit-testing a step | How is a step tested without a bus? | `hypha::testing::StepHarness` in `testing.h` (not in `hypha.h`): `set`, `setStale`, `setNeverArrived`, `setStagnant`, `step`, `sent`; `deliver()` also ages an `Arrivals` port by its newest sample, and `deliver()` / `deliverStatic()` give a `Transforms` port its transforms, looked up as in a started cycle (a buffer with no node: `hypha_transform_buffer_create_fed`, `hypha_transform_buffer_feed`, #248); an `optionalTopic` port is enabled in the harness when its table in the test's parameter file sets a `topic`, and `deliver()`, `set()`, `setStale()` and `setStagnant()` on a disabled input are refused; the harness moves no clock (`hypha::now()` in a harness step is the process clock; a test sets simulated time) | tests |
| Silence in a step test | Is a delivered input too old at this step? | `StepHarness::deliver(port, value, sentAt)`: judged at each `step()` by the `silence_ms` of the port's table, with the rule of the real capture (`value_usable_within`: one inclusive comparison); no `Arrivals` variant | tests |
| Golden-sample step test | How does a recorded MCAP fixture drive a step test, and how are its golden outputs read? | `hypha::testing::McapReplay` in `testing/mcap_replay.h` (not in `hypha.h`; link `hypha::testing_mcap`, the static MCAP reader `hypha_mcap_*` of crate `hypha_testing_mcap`): `feed(port)`, `stepDue()`, `next(harness)` steps on the epoch grid with the samples sent before each deadline, as a replay cycle; `read<M>(topic)` returns `Recorded<M>`; `feedIfPresent(port)` for an optional input that only some recordings hold; `endAt(time)` for the silent tail after the last sample | tests |
| Test on the bus | How does a test that uses the bus stay apart from other tests and from a running system? | `hypha_add_test(<target>)` (CMake, beside `hypha_generate_messages()`): the test runs in the domain `t_<pid of ctest>_<n>` under the test root `t_`; `hypha clean --domain t_` removes the test domains nobody uses | ex autoware, mrs, scan_cycle, sensor_pipeline |
| Latest input | What is the newest value of a topic? | `hypha::Latest`, `hypha::Input`, `cycle_latest.h`, `cycle_input.h` (it includes `cycle_sharing.h`, the home of `sharing()`); `*port` reads a usable value or throws `hypha::InputNotUsable`, `lastKnown()` reads an old one on purpose; `sentAt()`, `sequence()`, `publisherId()` read the frame header of the held value | app 6, mrs 8, ex 3 |
| Arrivals input | What arrived since the last step, in order? | `hypha::Arrivals`, `cycle_arrivals.h` (it includes `cycle_arrivals_merge.h`, the home of `visitArrivalsInOrder()`), the `arrival_budget` key of `[cycle]`; `sentAt(i)`, `sequence(i)`, `publisherId(i)` read the frame header of sample `i` | app 7, mrs 6 |
| Ports on one topic | How do several ports read one topic? | `hypha::sharing(port)`, `source.h` (internal), `hypha_input_source_create`, `hypha_input_source_destroy`, `hypha_cycle_bind_input` | app 1, mrs 1 |
| Topic fixed in code | How does an input read a topic that no parameter file names? | `hypha::Topic`, `topic.h` | mrs, tests |
| Output | How does a step send? | `hypha::Output::send(message)`, `cycle_output.h`, `hypha_cycle_bind_output`, `hypha_cycle_emission_reserve`, `hypha_cycle_emission_commit` | app 9, mrs 8, ex 4 |
| Step image | What did this step capture? | `hypha_cycle_image_*` | SDK |
| Port options | How is one port captured or sent? | Tags in the declaration (`optionalTopic`, `advisory`, `unsupervised`, `limitMax`, `hostClock`, `perStep`, `latched`, `maxPayloadDynamic`, `diagnostic`, a `hypha::Qos`), `options.h` | all cycle users |
| Service policy of a type | Which service limits does a topic of my message type get? | `hypha::Qos`, `hypha::QosOf<M>` (default `hypha::defaultQos`; `hypha::tfQos` for `tf2_msgs/TFMessage`; `hypha::imageQos` and `hypha::cloudQos` for a large message, named as a port option, never mapped), a `hypha::Qos` as a port option, `qos.h`; replaces `HYPHA_QOS` and the profiles | tests, ex 1 |
| Size of a large message | What bound do I name for an image or a cloud, and what does its pool reserve? | `hypha::serializedSize(message)` and `serializedSize(head, loanedBytes)` (`message_size.h`), `hypha topic pool`, the start-up line of a publisher; the rule in cpp-api.md §7.1 | camera and lidar driver ports (#241) |
| Foreign clock input | How does a simulated-time step read data stamped on another clock? | `hostClock`, `hypha_cycle_set_host_clock` | tests; mrs swarm on `spike/zenoh-swarm` |
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
| Silence limit | Is this input too old to use? | `silence_ms` (required on every input that counts for health), `stale()`, `unsupervised` (the code's way to run without one), `limitMax`, `hypha_cycle_bind_input_supervision`, `HyphaCycleBindingView.supervision` | app 3, mrs 3, ex 1 |
| Stagnation | Did the source behind a relay (driver, bridge, gateway) stop while the relay still sends? | `stagnation_ms`, `setAlive()`, `stagnant()`, `limitMax` | app 1 |
| Inputs of a step | May this step trust its inputs? | `Cycle::inputsOk()` | app 1 |
| Ignore override | Should this fault count right now? | `ignored`, `setIgnored()`, `cycle_supervision.h` | app 1 |

### 3.5 Time

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Framework time | What time is it for this node? | `hypha::now()`, `hypha_clock_now_nanos`, `hypha_base/time.h` | app 7, mrs 1, ex 4 |
| Host time | How much real time passed? | `hypha::steadyNow()`, `hypha_clock_steady_now_nanos`, `hypha_base/clock.h` | app 2, ex 1 |
| Step time | When is this step? | `CycleTime` (`now`, `stamp`, `dt`, `missed`); `hypha::now()` returns `now` while the step runs | all cycle users |
| Simulated time | Does this process follow `/clock`? | `HYPHA_USE_SIM_TIME` (`1`/`true`; `follow` is refused), `hypha_clock_*sim_time` | tests, tools, ex autoware |
| ROS stamps | How do I write or read a ROS `Header` stamp? | `hypha::toRosTime`, `hypha::fromRosTime`, `message_stamp.h` | app 9 |
| Seconds without a throw | How do I turn computed seconds into a `Duration` on a path that must not throw? | `Duration::tryFromSeconds` (empty for NaN or infinite seconds; `fromSeconds` throws) | tests; for the Autoware port's cycle steps (#126) |

### 3.6 Configuration

| Feature | Question it answers | API | Users |
|---|---|---|---|
| Parameters | What did the node's TOML file set? | `hypha::config()`, `ConfigView`, `config.h`, `hypha_node_config`, `hypha_config_value`, `hypha_config_list_len`, `hypha_config_list_value` | app 8, mrs 10, ex 7 |
| Scalar arrays | How do I read a list of numbers, flags or names? | `ConfigView::requiredArray<T>()`, `requiredArray<T, N>()`, `optionalArray<T>()`, `optionalArray<T, N>()`: each element is read as `required<T>` reads a value; no new C ABI function | none yet |
| Value checks | Is this value in range? | `hypha::constraints` in `constraints.h`, which `config.h` includes (`range(min, max)` with integer bounds compares an integer exactly, in C++), `hypha_param_validate_number`, `hypha_param_validate_nonempty` | app 4 |
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

| Item | Decision | State |
|---|---|---|
| Callback endpoints beside cycles | Keep both, with the rule of section 3.2. | decided |
| Where input limits come from | The port table, for every port: a composed `Cycle` port reads `silence_ms` and `stagnation_ms` from its table too. `InputOptions::silence`, `::stagnation` and the level-dependent key rules are gone. Code keeps `requireSilenceLimit` (the key is required) and `limitMax` (the bound the key must meet). `ignored = true` on a port with no limit is refused. | done |
| ROS stamps | `toRosTime` returns a stamp, not `std::optional`: a time outside the ROS range is refused where it enters, once (`std::out_of_range`). | done |
| Non-finite seconds | `Duration::fromSeconds`, `Time::fromSeconds` and `Duration * double` throw `std::invalid_argument` for NaN or infinite input instead of giving zero, and stay `constexpr` (#126). `Duration::tryFromSeconds` is the one non-throwing form, for a cycle step. The Rust `hypha_time` has no floating-point constructor or scaling, so it has nothing to refuse. | done |
| Several `Node`s in one process | The session is left by the last joined `Node`, not the first (#97): a count under the session mutex; a default-constructed `Node` does not count. The configuration is one per process too (#208): a later `Node` or `ConfigContext` naming no file shares it, the same file joins, another file is refused and the message names both files. A publisher whose node was destroyed refuses its loan and send with `InvalidState` (C ABI) and `publish()` returns false (C++). No new API. | done |
| Two threads on one `WaitSet` | Detected (#83): every call and every run claims the `WaitSet` with one compare-and-swap of an owner token and releases it when it returns, so the second of two threads gets `InvalidState` (FM-37, AoU-22). No new API: the claim is below the C ABI, and the existing calls answer. The run-owner mutex it replaces is gone. | done |
| Transform lookups that wait | No (#91, 2026-11-22). A lookup never waits: the buffer fills only while the node runs, on the run's thread, so a waiting callback would block the node's other callbacks and, under replay, the clock it waits for. A node that must have the transform holds its message and retries in a later callback (what `tf2_ros::MessageFilter` does); a ported node's shim does that, not the core. | decided |
| Time going back | Only the restart design: a timer or cycle that sees simulated time go back, and a node that reads a `/clock` lower than its last one (#130), end the run with `ClockRewound` and the process restarts. A host clock step back is not a rewind and restarts nothing. The in-place handling is gone (`TriggerSchedule::rewind`, `pending_rewind`, `CycleTime::rewound`, the TF buffer's `clear_dynamic`). FSR-37 says so. | done |
| Open-loop playback | Yes, the one alternative to the lockstep, for a whole launch (#102, #147, owner 2026-11-24): `HYPHA_REPLAY_LOCKSTEP=0` turns the lockstep off for the player and every node; nothing puts one node of a launch outside it (`HYPHA_USE_SIM_TIME=follow` is gone). The player sends `/clock` and the data on the `--rate` schedule, reads no report and refuses `--rate max`; a node follows `/clock`, reports nothing, and a lost replay input is warned of, not failed. A rewind still restarts every node (FSR-37). Not deterministic, and documented so (`replay-flow-control.md` §4.6). It removes nothing: the lockstep stays the default. | done |
| Plain callbacks in a replay | No (#153, owner 2026-11-24; #72 withdrawn): the lockstep runs rate and triggered cycles only. A plain subscription or timer runs as its sample arrives with framework time the clock, reports nothing and is waited for by no one; a node with only plain callbacks opens no `/clock_progress` publisher, so it is no cycle's upstream. A system that cannot be built from cycles plays in the open loop (`execution-model.md` §11.2). | done |
| Low-level APIs | Section 3.12. The C++ `WaitSet`, the lazy `publish(builder)` with `skipWhenNoSubscribers`, `publishedCount`/`suppressedCount`, `creationStatus`, `waitForSubscribers`, the subscriber `health()`/`metrics()`, `describe()`/`domain()` (`introspect()` answers them), `Node::hasCycles`, `Cycle::release`, `Cycle::setArrivalBudget`, `_hz`, `Duration::fromChrono` and `toString` are gone: no user outside tests. | done |
| Reporting channels | The node state is hypha's: the library judges the required inputs (`hypha_node_state`), and `runNode` publishes `~/state` and `~/introspection`. hypha renders no manifest text: an application lays out and logs the manifest table from `started(Node&)`. Introspection answers "what is this node made of", the state "are its inputs healthy", metrics "how many of X", cycle and binding health the counters of one cycle. Left: merge the counters that two channels both report. | done (2026-11-12) |
| `hypha::now()` in a step | One step time: `CycleTime::now`, with `stamp` as its ROS stamp. `hypha::now()` returns it for as long as the step runs, so code that cannot reach the `CycleTime` is right too, also in a replay step that catches up. `scheduled`, `callbackStart`, `captureSteady` and `messageStamp` are gone. | done (2026-11-12) |
| `Rate` and the clock sleeps beside `Node::timer` | `Node::timer` is the one way to run code periodically outside a cycle. `hypha::Rate`, `hypha_rate_*` and the clock sleeps are gone; `large_source` paces its publishes with a timer. | done |
| Publisher payload default | What limit does a publisher that names none get? The library decides from the definition, below the C ABI, for every SDK and relay: it checks the definition against the MD5 (a mismatch is refused), bounds a fixed-size type to its exact encoded size whatever was asked (a bound has no effect on it), and leaves any other type growable: no byte limit unless the endpoint names one (`HyphaQos::max_payload_dynamic`). Type and package rules do not exist. `hypha play` bounds each topic to the largest message of the recording. Replaces the C++ `payloadLimitFor` and the hand-written `/clock` size. | done |
| Growable first slot size | How large is a growable publisher's pool before any message grows it? Its first slot size times the pool samples: 4096 bytes unless the endpoint names `HyphaQos::initial_payload` (C++ `Qos::initialPayload`), a hint that is not a bound and not compared between endpoints. A larger message grows the segment; iceoryx2 is patched to 32 growths (`vendor/README.md`). | done |
| QoS policy | Which service limits does a topic get, and who decides them? The topic's creator declares a `QoS` policy (`HyphaQos`: a name, `max_publishers`, `max_subscribers` and `max_depth`); the payload bound is per endpoint. A publisher that declares nothing creates a topic with hypha's one built-in policy, `default` (8 publishers, 16 subscribers, depth 32), and a framework topic (`/clock`, `/clock_progress`) gets the framework's limits (32 / 32 / 32). A subscriber that declares nothing never creates a topic, except a framework topic: on any other topic it is refused with `NotAvailable` until a publisher or a declaring endpoint has opened it. An undeclared subscriber, a tool or a bridge, may hold three slots beyond the policy's `max_subscribers`, which declared subscribers never give up. The creating endpoint stamps the policy on the service as `hypha.qos=name/publishers/subscribers/depth`. An opener that declares a different policy is refused with `QosMismatch`, and the log names both; an opener that declares nothing adopts the stamp. A policy declared on a framework topic neither creates nor refuses it: its limits are fixed. A service whose stamp is missing or names no policy of this build is refused with `QosMismatch`. A subscriber asks for its own depth: depth 0 is the service's depth, and a larger one is refused with `ResourceLimit`. | done |
| Large messages: sizing and policy (#241) | How does a developer size an image or a cloud, and which policy do both ends name? The rule is in cpp-api.md §7.1: slots = (subscribers + 3) x (depth + 2) + 1 + loans, bytes = slots x (message + 104), the one rule `qos::pool_samples` in Rust. The numbers need no code read: `hypha::serializedSize(head, n)` (C++, the size of a message whose array holds n bytes) and `hypha topic pool` (the pool of a policy; no bus, no new C function, `abi.revision` unchanged); a publisher whose pool is a megabyte or more logs it at start, a bounded one that takes more than half of `/dev/shm` warns. `hypha::imageQos` and `hypha::cloudQos` are options of a port, not `QosOf` mappings: a type is mapped once per application (two lidars could not differ, issue 241), and `hypha play` or a bridge creates a topic with `default` and would refuse the typed reader that came after. The `default` policy is unchanged: raw creators and unaware code rely on its room. | done |
| Cycle output limits | Who owns the per-step message count and payload limit of an output? `hypha_cycle`: `CycleRuntime` hands the step an `EmissionBudget` that admits each message (`hypha_cycle_emission_reserve` only marshals its status), and the dispatch checks the staged list against the same limits. `hypha_ffi` keeps no copy. | done |
| ABI folds left for the next wave | The pre-commit setters (`hypha_cycle_set_class_based`, `_set_trigger`, `_set_clock_driver`, `_expect_participant`) stay separate: the participant list is variable-length, and the C++ SDK sets them from three different places, so one struct would not be simpler for it. `hypha_node_record_config` and `hypha_config_record_config` could be one call once the registry reads rows from the config. `hypha_waitset_create`/`_destroy` are gone: a node has one `WaitSet`, `hypha_node_default_waitset`, which the node destroys. | open |
| Composed cycles beside `CycleNode` | A node has one `CycleNode`: its cycle has the node's name, reads `[cycle]` and is started by `runNode`. A second step in the node is a composed `Cycle` with its own name, started in `setup()`. | done (2026-11-12) |
| Queued-work limits | Two knobs: `perStep(n)` per port (default 8) and `arrival_budget` in `[cycle]` of a `CycleNode`. `Cycle::setInputDefaults(n)` is gone; a port that needs another share sets its own. | done |
| Input loss for applications | posmaster adds `overrun_total`, `work_limit_drops_total` and `age_rejections_total` of `bindingHealth()` into its own "transport loss", although an age rejection is no transport loss. An application should not re-derive a judgement from raw counters; part of the reporting revision. | open |
| Staging a fault in a unit test | Replaced by `hypha::testing::StepHarness` (`testing.h`): `stage()`, `stageFaults()` and `Output::lastSent()` are gone, and `send()` outside a step throws. | done (2026-11-15) |
| One way to create an endpoint | `Node::advertise`, `Node::subscribe` and `Node::timer` return the endpoint and throw a `std::runtime_error` that names the port and the status (`ConfigError` for a configuration fault). The builders, the public endpoint constructors, the `enabled` options and `fromParam` with its fallback are gone. A bare string names a port table; a topic that code fixes is `hypha::Topic{...}`. | done (2026-11-12) |
| Topic rules | A port table's topic is required unless the port is `optionalTopic` (an absent table or `topic = ""` disables it). `Switchable` is gone; an application no longer checks its required topics by hand. | done (2026-11-12) |
| Several triggers (#206) | One triggered cycle steps on any of its trigger ports: `setTrigger` adds a port, `hypha_cycle_set_trigger` once per port (`abi.revision` 7: the call no longer replaces the trigger). One step per sample, in send-stamp order, a tie in port order; the other trigger ports hold no sample in it. Every trigger has its own `silence_ms`; the fallback runs after the shortest, and only the ports past their own limit read stale. Under simulated time a step at T waits until every upstream reported T. `usable()` is on every input port (`Input<M>`), not only on `Latest`. `IntrospectedCycle.triggers` lists the trigger topics. Nothing new answers "which port fired": `samples().empty()` does. It removes the overwrite of a second `setTrigger`. | done |
| Reading and sending in a step | `Latest`: `has()`, `*port`, `usable()`, `fresh()`; `valueOr(fallback)` replaces the buffer that read as a default message before the first value. `Arrivals::samples()` yields messages; `Sample<M>` is gone. `Output::send(message)` replaces the public `message` buffer. `InputVerdict` has three values; a capture without a new sample is a usable Arrivals input. | done (2026-11-12) |
| Several ports on one topic | `hypha::sharing(port)`: the ports share one subscriber, each with its own limit keys, in any start order. `hypha::Source` and `createSource()` are gone, so ports are plain members and a node with shared inputs is a plain `CycleNode`. | done (2026-11-12) |
| One name, and the rate from the file | A `CycleNode` takes no name. Its rate is `rate_hz` of `[cycle]`; `setRate()` is for a rate that code computes, and both together refuse the start. | done (2026-11-12) |
| Lifecycle hooks | `prepare()` is gone: the constructor reads configuration before the node joins the bus and `setup()` creates endpoints. `started(Node&)` runs once after the node report, when the manifest is complete; an application can log its manifest table there. `runNode` forwards its extra arguments to the constructor. | done (2026-11-12) |
| Status in C++ | `hypha::Status`, with `name()`; `require()` and the start errors print the name, not the number. `TransformError`, `InputVerdict` and `StopReason` have `hypha::name()` (#100, 2026-11-22). Application code names no C type: `hypha::BindingHealth`, `hypha::CycleHealth`. | done (2026-11-12) |
| Port options | One way to configure a port: typed tags in its declaration, checked at compile time per port kind (a wrong or repeated tag does not compile), each also `tag(bool)` for a run-time value. Inside a `CycleNode` the port kinds and tags are names of the class. `InputOptions`, `OutputOptions`, `PublisherOptions`, `SubscriberOptions`, `TopicRule`, `LimitRange`, `OutputFailure`, `QueueOverflowPolicy`, `setOptions()` and `setTopicParameter()` are gone; a port whose options or name come from configuration reads it in its member initializer. `optionalTopic` (may be unwired) and `advisory` (does not count for health) stay two tags: an application uses both mixed combinations. Gone without a user: the subscriber `queueSize`, the per-endpoint no-publisher delay, the overflow policy option (a queued port drops its oldest work), the `overflow_policy` table key (it silently overrode the code), the `hypha_param_topic` fallback, `hypha_node_now` and the Rust SDK `BusNode::topic`/`topic_or`. Also gone: `HyphaCycleBindingOptions.overflow_policy`, the `drop_newest` capture and the `IntrospectedInput.overflow` field with its `OVERFLOW_*` constants; `carry` stays below the C ABI as the capture of a trigger. `hypha::Port` and `hypha::Input<M>` are public, so a plugin host owns ports without `hypha::detail`. | done (2026-11-12) |
| Dead C ABI items | What goes when nothing in this repository calls it? `hypha_waitset_find_cycle`; `HYPHA_ATTACH_INTERVAL` (kind 1 is refused, not reserved; a timer, C++ `Node::timer`, is the one periodic registration); the `reason` parameter of `HyphaSubscriptionCallback` and `HYPHA_REASON_DATA` (a subscription callback is `(delivery, context)`); `HyphaCycleBindingOptions.overflow_policy` with `HyphaCycleOverflowPolicy`, and `IntrospectedInput.overflow`. Below the ABI: the byte limit of `CycleHistory` (samples bound it), the Rust SDK `SchemaError::Panic` and the bridge's `BridgeError::WorkerPanic` (a panic aborts, or a worker's panic goes on in the joining thread). `hostClock`, `class_based` and the arrival budget stay: C++ uses them. | done (2026-11-14) |
