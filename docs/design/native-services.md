# Native services

Design for #132: typed request and response between nodes, from a ROS 1 `.srv`.

Status: accepted design, after an independent review whose seventeen changes are folded
in. Stage 1a (the transport) is implemented in `crates/hypha_iceoryx2/src/service/`; the
runtime, the C ABI and the C++ endpoints of stage 1b in `crates/hypha_node/src/service/`,
`crates/hypha_ffi/src/service.rs` and `crates/hypha-cxx/include/hypha/service*.h`. The
call log (`crates/hypha_node/src/service/call_log.rs`), the servers' definitions in the
schema store and `hypha service list|info|call` (`apps/hypha_tools/src/bin/hypha/service.rs`)
complete 1b; the stages above are not built. Decisions of the second part where this
document is silent: the log's topic is `<service>/calls` under a policy `serviceCalls`
(the service's clients and tools as publishers, depth 4); a record carries at most 2 KiB
besides the policy's request and response bounds, its reason cut at 1 KiB; `call` with no
body calls a request of no fields and prints the template of any other; the exit codes
are 0 answered, 1 a tool failure, then 2 to 6 for refused, no server, not sent, server
lost and timeout; `list` shows no per-outcome counts before the `~/state` rows of stage
2, and the paused-clock notice is not built.

Stage 1c is in two parts. The first is built: the cycle ports `Requests<Srv>` and
`Calls<Srv>` (`crates/hypha_node/src/service/cycle.rs`, `crates/hypha-cxx/include/hypha/cycle_service.h`),
the strict cut, the two upstream edges, the §6.6 amendment, and the lockstep tests of
section 11 (`apps/hypha_tools/tests/service_lockstep.rs`, with the C++ participant
`crates/hypha-cxx/test/service_lockstep_peer.cpp`). `StepHarness` support and
`hypha::Sequence` are the second part; until then a `StepHarness` step that touches a
service port throws `std::logic_error`. Decisions of the first part where this document
is silent:

- **A service edge bounds which steps run, not the progress a triggered cycle reports.**
  A request or a response steps no cycle (stage 3 is not built), so a triggered cycle
  reports from its topic upstreams, its pending triggers and its fallback alone; a step
  held by a service peer stays pending and keeps the report below it. Counting service
  edges in the report would make a client and its server lift each other's report one
  nanosecond per turn.
- **A client and its server in one `WaitSet` hold each other.** The strict cut alone does
  not make their order irrelevant (section 4 said it does): a turn runs all the due
  deadlines of one cycle before the next cycle, so with several deadlines per turn a server
  registered first ran a deadline before its client ran the step that called it, and what
  a step read depended on `--chunk`. Each cycle with a service peer in its `WaitSet` holds
  a step until the peer has run every step before it, as for a peer in another node, and
  the turn repeats the timers of such cycles while that lets a held step run. The lockstep
  test of a pair in one node fails without it. Two cycles of one `WaitSet` joined by a topic use the
  same hold (#307, `waitset/local_edges.rs`).
- **A step that sends no output sends no answer and no call.** A step that stops the run,
  or continues with a status, publishes nothing (execution-model.md §6.1), so it is treated
  as a failed step here too: its requests end `ServerLost` (`server step failed`), its
  calls `NotSent`.
- **The answer to a deferred request given during any cycle step on the run's thread
  leaves after that step**, with its outputs, and a failed step answers it as failed, as
  section 2 asks of a deferred request answered in a later step.
- **`reply()` in a step reports `TooLarge` at once** for a response above the bound; the
  client is refused with the sizes after the step, as for a plain server.
- **Outcomes reach a step in call order**; an outcome no step read is dropped after it,
  counted, and logged once per port at info level.
- **The peers outside the replay** (review of part 1): a request or an answer of a peer
  that reports no progress is read at the first step after it came, not by its stamp, so a
  tool can call a cycle server during a replay and a cycle client gets the answer of a plain
  server instead of a `Timeout` with the answer in hand. A cycle client names itself
  `<index>/<cycle>` to its server (at most 64 bytes), so two cycles of one node on one port name do not tie;
  the client's per-run id breaks a tie left only by a node whose name cannot be read.
- **Outcomes a failed step did not read go to the next step**, as its `NotSent` calls do;
  a successful step that does not read one drops it, logs at warn once per port and counts
  it (`Calls::unreadResults()`, C `hypha_cycle_unread_results`; the `~/state` row is stage 2).
- **A step that fails on replay fidelity** (a lost input) has already sent its answers and
  calls, as it has sent its outputs: the fidelity check follows the sends.
- **`per_step`** is at most the server's slots (`clients × outstanding`); a `Calls` port's
  `outstanding` defaults to 1, as section 8.6 says, and counts the calls staged in the step.
- **ABI**: six functions (`hypha_cycle_bind_server`, `hypha_cycle_bind_client`,
  `hypha_cycle_requests`, `hypha_cycle_next_result`, `hypha_cycle_call`,
  `hypha_cycle_unread_results`), coarse as those of 1b (#276); revision 12, the unreleased
  revision of 1b.

Each decision gives its reason. Where the author checked a change
against the code and kept a different mechanism, an **author's note** says so, with the
evidence.

Readers: the owner, who decides, and whoever implements the stages of section 11.

## 0. Why now, and what the design must serve

Every port that needs a call builds one out of two topics, and each builds a different
one (issue #132):

- `mrs_bridge/Call` in `examples/mrs/host/include/mrs_host/service.h`;
- `hypha_rclcpp_msgs/ServiceCall` in the Autoware shim;
- a `request_id` field added to `nav_msgs/GetMap` in the `map_server` port, which changes
  its MD5.

The vetting reports in the playground show where services stop a port:

| Port | Services | Without them |
|---|---|---|
| MRS UAV system | 10 servers (4 `control_manager`, 3 `LandoffTracker`, 1 `uav_manager`, 2 simulator) and 19 clients (mission 9, teleop 5, `uav_manager` 5): arming, offboard, `toggle_output`, `switch_controller`, `switch_tracker`, takeoff, `goto` | The flight. These services are the control interface of the system. |
| Autoware | the EKF's `trigger_node_srv`: the node publishes nothing until it is called with `true` | The estimate. `gnss_pose_initializer` is a workaround. |
| `arena_camera` | seven setters (`set_exposure`, `set_gain`, ...) and `set_camera_info`, called by operators | Dropped from the port. |
| `map_server`, `diagnostic_aggregator` | `static_map`, `change_map` | Waiting, or a third wire format. |

Two shapes cover all of it.

- **A setter or a switch:** the handler answers at once (`switch_tracker`, `set_exposure`,
  `arming`, `trigger_node`).
- **A sequence:** the handler starts work, calls other services, and answers when done. In
  `uav_manager` takeoff: switch the controller, switch the tracker, wait 0.3 s, call the
  tracker's takeoff, answer. ROS 1 runs the whole sequence inside the callback, blocking a
  spinner thread on each nested `call()` and on `ros::Duration(0.3).sleep()`
  (`mrs_uav_managers/src/uav_manager.cpp:1480-1590`).

## 1. What a service is

**Decision.** A service is a name and a `.srv` type. A server answers each request once.
A client sends one request and gets exactly one outcome.

A name resolves as a topic name does: absolute, relative to the node's namespace, or `~/`
private, from a port table or a literal. A stream is the name plus the MD5 of the `.srv`
type, as a topic stream is the name plus the message MD5 (PR #266). **A name has at most
one server, across all its versions.**

*Reason.* A ROS 1 porter expects one name and one type. The `.srv` MD5 already exists:
`hypha_msggen` emits `Request`, `Response` and the service type with ROS 1's service MD5
over both halves (`hypha_msgdef::resolve::service_identity`), and FSR-21 already requires
that generation.

The stream key lets clients of two versions coexist and get a clear answer. One server
per name keeps the rule ROS 1 users rely on: one node answers `/uav1/arming`. Two servers
of two versions of `arming` would be two sources of truth for one command.

| Situation | Behaviour |
|---|---|
| A second server on the name, same version or another | Its creation fails with `ResourceLimit`. The message names the holder: `hypha: service '/uav1/arming': already served as std_srvs/SetBool (09fb0352) by 'hw_api' (pid 4711)`. |
| The holder is a dead node | The new server reclaims the dead node, as that node's own restart would. It logs `hypha: service '<s>': reclaimed dead node '<n>' (pid <p>) that held its server`, and then creates the server. A live node cannot be taken over: iceoryx2's monitor lock decides which nodes are dead. The precedent is the reclaim of dead CLI nodes by any node (`hypha_core/src/port/spec.rs:362-366`). |
| A client starts before its server | Not an error (FSR-25). A call made while no server exists stays pending inside the client, using one of its outstanding slots. It is sent when a server appears, which is checked on the graph event and at each turn. It ends `NoServer` only at its deadline. |
| Only a server of another version exists | The call ends `NoServer` at its deadline. The client reports the "other versions" fact of topic overloading: `hypha: service '<s>' is served as <T> (<md5>), not as <T> (<md5>)`. |
| The server exits normally during a call | The outstanding calls end `ServerLost` at the client's next turn. Destroying the server ends every slot of its table (section 2). |
| The server crashes during a call | `ServerLost`, found by the liveness probe at the call's timeout (section 5). |

`client.ready()` says whether a live server of the client's stream exists now.
`client.exists()` is the ROS 1 spelling of the same call. `client.waitForServer(timeout)`
waits for one; like `callAndWait`, it is refused on a dispatch thread (section 3).

## 2. The server's execution model

**Decision.** There is one mechanism with two front ends.

- **Underneath** the server owns a fixed table of `clients × outstanding` slots, each
  holding one received request. A request event wakes the run; no timer and no poll is
  involved.
- **A cycle** reads requests as an input port, `hypha::Requests<Srv>`, with the cycle's
  capture, budget and replay rules. The step answers through the handle it is given.
- **A plain node or `ApplicationNode`** registers a callback,
  `node.advertiseService<Srv>(name, handler)`. The callback runs on the `WaitSet` thread
  between turns, as a subscription callback does.

*Reason.* The MRS managers are cycle nodes. `control_manager` is a triggered cycle, and
its four servers already answer inside its step, through `Arrivals` of request topics
(`examples/mrs/managers/control_manager.cpp:429-470`). A server that ran beside the step
would bring back the concurrency that cycles remove: a `switch_tracker` callback that
changes the active tracker between two steps that read it.

A camera or a map server is a plain or application node with no cycle, and a callback is
the natural form there. One table underneath keeps the integrity, the limits and the
counters in one place in Rust (FSR-19). The cycle port is in the first version, not a
follow-up, because the flight runs on cycle nodes.

**Answers.** A handler gets a `hypha::Request<Srv>&` and does one of three things with it:

- `reply(response)` answers it.
- `refuse()` or `refuse("why")` gives the client `Refused`, as a ROS 1 callback does when
  it returns `false`.
- `defer()` returns a `hypha::Deferred<Srv>` to answer later.

A request that a step or a callback **read** and left without any of the three is
refused, counted (`service.unanswered`) and logged once. A request that a step **never
read** stays queued for a later step. A step that never calls `port.requests()` therefore
refuses nothing. That case is bounded by `per_step` and by the client's timeout. Without
this rule, the quick-start idiom `if (!speed.usable()) return;` would silently refuse
every call.

**`Deferred` is a handle into the server's table**, not the request itself. A deferred
request stays in its slot, and `Deferred` holds the slot index and a weak reference to the
server.

*Reason.* In iceoryx2 0.10.1 an `ActiveRequest` holds an `Arc` of the server's shared
state (`active_request.rs:95`), and the server's slot is released only when that state
drops (`port/server.rs:211-218`). A `Deferred` that owned the `ActiveRequest` and outlived
the server would keep the server registered, so its clients would never see
`ServerLost`, and a restart of the server in the same process would be refused.

| `Deferred` | Rule |
|---|---|
| Construction | Default-constructible and empty. `request.defer()` fills it. Move-only. `explicit operator bool` says whether it holds a request, so no `std::optional<Deferred>` is needed. |
| `reply(response)`, `refuse(...)` | Answer the slot. Afterwards the handle is empty. |
| Destroyed while holding a request | Refuses it, logged once. |
| Its server destroyed first | Every slot ends, and clients see `ServerLost`. A later `reply()` returns `Disconnected`. |
| The client gone | Each turn the server drops the slots whose client is gone (`ActiveRequest::is_connected()`) and counts them as `abandoned`. A later `reply()` returns `Disconnected`. |
| Answered from another thread | Allowed. That thread sends at once, stamped with the process's framework time. Such an answer is not part of a deterministic replay. |

Bounds: the server holds at most `clients × outstanding` requests, deferred ones
included. iceoryx2 sizes a client for one more generation of requests that the server
still holds after the client dropped them (`static_config/request_response.rs:103-125`).
A client whose calls timed out while the server still holds them could therefore exceed
that, and its send would fail with a transport error that no outcome names. So a `call()`
that would exceed the limit returns `Busy`, and `describe()` says
`the server still holds N expired calls of this client`.

**In a step, answers leave after the step**, in order with its outputs. A step that fails
sends none of its outputs (FSR-38).

A request that a **failed** server step read ends `ServerLost` with the reason
`server step failed`. It does not end `Refused`: C++ state that the step changed before it
threw is not rolled back, so the call may have had an effect. A handler that throws ends
`ServerLost` the same way, and the throw counts as the server's `Panic`. A deferred handle
that is answered in a later step follows that step's rule.

**Not chosen.**

- A handler that returns the response (`Response handle(const Request&)`). It cannot
  defer, so the sequence shape would need a second form. One handle form serves both, and
  `request.reply({...})` is as short.
- Long-running work in a service. ROS 1 guidance says to use an action for it. A deferred
  answer closes a short sequence; it streams no progress.

## 3. The client's execution model

**Decision.** Inside a run, a call is asynchronous, and its outcome arrives later.

- **In a cycle,** a `hypha::Calls<Srv>` port is both the output and the input of calls.
  `call(request)` in a step stages the request, which is sent after the step succeeds, as
  an output is. The outcome is an input of a later step: `calls.result()`, or
  `calls.results()` when more than one may be outstanding.
- **In a plain node,** `client.call(request, onResult)` sends at once. `onResult` runs on
  the `WaitSet` thread at a later turn, never inside `call()`.

`call()` returns `[[nodiscard]] hypha::CallStatus`, one of `Sent`, `Busy` or `TooLarge`.
It refuses only what it can know locally:

- `Busy`: the client is at its outstanding limit. That includes calls that the server
  still holds after they timed out (section 2).
- `TooLarge`: the request is above the service's bound.

Every `Sent` call ends in exactly one outcome, delivered in exactly one step (or one
callback):

| Outcome | When | `mayHaveRun()` | `safeToRetry()` |
|---|---|---|---|
| `Answered` | the server replied. `ok()` is true | yes | no |
| `Refused(reason)` | `refuse()` or `refuse("why")`; a `Deferred` destroyed unanswered; a response above its bound; a request that a step read and did not answer | no | yes |
| `NoServer` | no live server of the stream until the timeout | no | yes |
| `NotSent` | the client step that staged the call failed, so nothing was sent | no | yes |
| `ServerLost(reason)` | the server crashed or exited; a handler threw; a server step failed; the response did not verify. The reason is in `describe()` and in the counters | yes | no |
| `Timeout` | no answer within the timeout | yes | no |

An outcome that the step does not read is dropped at the end of that step and counted
(`results_unread`). A port used to fire calls and forget them therefore does not wedge.
`client.pending()` says whether a call is outstanding.

**The timeout** is a field of the service's policy (section 6), 1 s by default. Code
(`hypha::timeout(d)`) or a client's table (`timeout_ms`) overrides it per client.
`describe()` names where the value came from: `timeout after 1 s (policy
defaultService)`. It is counted in the client's framework time, so a replay times out
the same calls on every run (section 4).

**A blocking client is its own kind:** `node.serviceClient<Srv>(name,
hypha::blocking)`. It is never attached to a `WaitSet`, and it offers `callAndWait(request)`
and `waitForServer(timeout)`. The caller's thread waits on the client's own response
event. The library starts no thread, so FSR-14 holds.

These calls are for a tool, a test, or a worker thread of an `ApplicationNode`. On a
thread that is running a `WaitSet` dispatch they are refused with `InvalidState`, and the
message says what to write instead: `hypha: callAndWait on the run thread would wait for
the thread that must deliver its answer; use a Calls port in a cycle or call(request,
onResult) in a callback`. The check is made at run time, because C++ cannot see "inside a
dispatch" at compile time. Schema lookup inside a callback already follows the same rule
(execution-model.md §5.1).

*Reason.* A blocking call in a step stalls every other input of the node. With its server
in the same `WaitSet`, it can never be answered. Under lockstep it holds the replay until
`--timeout` (replay-flow-control.md §6.7).

The common ROS 1 pattern is "in a callback, call another node's service and use the
result". In a cycle it becomes a state over steps, written by hand or with `hypha::Sequence`
(section 8.2). In a plain node it becomes a continuation.

## 4. Time, lockstep, recording, replay

**Stamps.** A request carries the client's framework send time in its frame header
(`framework_sent_nanos`), as a topic frame does. For a staged call, that is the time of the
step that staged it. A response carries the time of the server's step or callback that
answered it.

**Lockstep (decision).** Requests are an input of the server's cycle. A step at deadline
D takes the requests stamped strictly before D; a trigger or fallback step at T takes
those stamped strictly before T. They are ordered by stamp, then client node name, then
client port name, then sequence.

Every participant client is an upstream of the server (§6.4 of replay-flow-control.md),
and every participant server is an upstream of its clients. Both edges are cut strictly.
A client and its server therefore form the two-node feedback loop of §6.4, which does not
deadlock.

execution-model.md §6.6 item 5 changes with this. A triggered step at T needs an upstream
that feeds only non-trigger inputs (queued inputs, requests, responses) to report T − 1 ns,
not T, because those inputs are already cut strictly before T. This removes the deadlock
of a triggered client that calls a triggered server.

*Reason.* This is the cut every other input uses, so the served step depends only on
stamps. An earlier draft admitted requests at `/clock` sample boundaries. Under that rule
the served step depended on things outside the system under test:

- on `--chunk`;
- on attached observers, because the player shortens a chunk to the smallest subscriber
  buffer over all subscribers, `hypha topic echo` and Foxglove included;
- on `/clock` resends during a wait, and so on host load;
- on `--loop` and on backward seek;
- on the per-run random client ids of `next_publisher_id`.

A timeout tuned live would also have expired in replay only. All of that is gone. The tie
order uses names, which are the same on every run, not ids.

**Timeouts under simulated time.** A call sent at S times out at the client's first step
with D ≥ S + timeout. That is deterministic, with a resolution of the client's period.

**Order inside one `WaitSet`.** A client and its server may be cycles of one node. The
strict cut keeps a request stamped D from a server step at D; and each of the two holds a
step until the other has run every step before it, as a peer in another node is held, so
a turn that runs several deadlines serves the request at the same step (status note).
Stage 1c tests this.

**Remaining non-determinism, to document:**

- a client or a server that is a plain callback, which takes no part in lockstep
  (execution-model.md §11.2). The cut binds only the peers that report progress: a cycle
  server serves the request of a tool (`hypha service call`, stamped with the host's time)
  or of a plain node at its first step after the request came, and a cycle client reads the
  answer of a server that does not report at its first step after it came, whatever their
  stamps; each port logs this once;
- a client and a server that are cycles of two `WaitSet`s of one node: they report as one
  node, and no hold orders them; the cycle logs it once;
- a `Deferred` answered from a worker thread;
- a call made before a non-participant server started;
- the first chunk of a replay without `--nodes`.

**Recording (decision).** `hypha record` does not record the request-response services.
It follows publish-subscribe services only, so a recording can never call a live server.
That removes the replay hazard of the topic pair, where a recorded `trigger_node` request
started the live EKF again.

What is recorded is the **call log** of stage 1b. Each client publishes
`hypha_msgs/ServiceCall` on a topic in the service's namespace, carrying:

- the service, the client node, the port and the sequence;
- the outcome;
- the sent and done stamps;
- the request bytes and the response bytes, bounded by the policy.

It publishes only while someone subscribes, which costs one atomic read per call.
`hypha record` records the log by default, `hypha play` never republishes it, and
`hypha top` reads it.

**Replay (decision).** `hypha play` never makes or answers a call by default. Two later
modes read the call log:

- `--serve <service>`: the player answers a client under test in recorded order.
- `--call <service>`: the player makes the recorded calls into a server under test and
  reports differing responses.

Neither is in stage 1. A replay of a system whose participants call each other repeats
exactly by the lockstep rule above, because the calls are made again live.

## 5. Integrity and failure semantics

**Frames (decision).** Both directions carry the full frame header and its checks, and the
header layout is unchanged (communication-integrity.md §2, FSR-09/10). The CRC's start
value binds each frame to its role:

- A request is sealed for `<service>␟request`.
- A response is sealed for `<service>␟response␟<client id>␟<request sequence>`.

`␟` is the unit separator (U+001F), a control character that no topic or service name
may hold, so no topic frame and no frame of another service is sealed with these codes.

So a response verifies only against the request it answers. It cannot pass as a request,
as a topic frame, or as the answer to another call. Stage 1a tests all three.
FSR-03's overhead stays 104 bytes.

**Sequences.** A request carries its client's own sequence. A response carries the
sequence of the request it answers, not a counter of the server's: one server counter
across all its clients would look like a gap to each of them. Responses therefore count no
`sequence_gap`, and requests count no gaps either. A client may legitimately leave numbers
unused, for example a call that ended `NotSent`.

**Answer kinds.** The response flags carry the kind, inside the CRC: an answer, a refusal
(`FLAG_REFUSED`; the payload is the UTF-8 reason), or a failure (`FLAG_FAILED`, for a
handler that threw or a failed step; the payload is the reason). The transport then
reports `ServerLost(reason)`.

A hypha server drops its `ActiveRequest` when it answers, and a hypha client drops the
pending call when it takes the first response. Either drop closes the call's channel, so a
second response has nowhere to go.

| Case | Client outcome | Detected by |
|---|---|---|
| No live server of the stream until the timeout | `NoServer` | iceoryx2 reports 0 server connections at each send attempt |
| Outstanding limit, expired calls the server still holds | `call()` returns `Busy`; nothing sent | the client's count of its own and its held calls |
| Request above its bound | `call()` returns `TooLarge` | encode |
| Response above its bound | `reply()` returns `TooLarge` to the server. The client gets `Refused("response N B above bound 4096 B, policy defaultService")` | the server |
| Server refuses, or a request read and not answered | `Refused(reason)` | the server, flag `FLAG_REFUSED` |
| A handler throws, or a server step fails | `ServerLost(reason)` | the server, flag `FLAG_FAILED` |
| Server exits or destroys its endpoint | `ServerLost`, at the next turn | iceoryx2 disconnects the pending call when the server's table is dropped |
| Server crashes (SIGKILL) | `ServerLost`, at the call's timeout | iceoryx2 alone keeps such a call connected. At a timeout of a still-connected call, the client probes the server's node once (below) and reports `ServerLost` instead of `Timeout` |
| Server restarts | New calls reach the new server. No old request is served again | the restart, or a new server, reclaims the dead node, which disconnects the old calls |
| No answer within the timeout | `Timeout`. A late response is dropped and counted (`late_responses`) | the client, in framework time |
| Response corrupt, misrouted, or of another schema | `ServerLost("invalid response: <check>")`, counted in the frame counters | the frame checks |
| Request corrupt, of another schema, or above its bound | `Refused("invalid request: <check>")`, counted as `invalid_requests`; never a disconnect that reads as the server leaving | the server's frame checks |
| Client gone before the answer | The server drops the slot and counts it as `abandoned`. A later `reply()` returns `Disconnected` | iceoryx2 `ActiveRequest::is_connected()` |
| A server queue that is full | Cannot happen. A request is never dropped silently | iceoryx2 sizes each client's queue to its outstanding limit (`port/server.rs:437`); the client counts its held calls |

**The liveness probe** runs only at a timeout of a call that is still connected, and at
most once per service per second, on the run thread. Its cost was measured in the spike:
0.55 to 1.8 ms per probe at load average 51.

*Author's note (change 16).* The review names `Node::state_of`, which scans every node of
the domain with filesystem calls. The probe uses the service's own node list
(`PortFactory::nodes`, `service/port_factory/request_response.rs:173`), which visits only
the nodes registered at that service. That is the per-stream form PR #266 adopted for the
same reason. The rate limit is kept as the review states it.

*Review of stage 1a.* A probe's result is reused only within its second and only while
the stream's server is still the node it judged. Before, a server killed and replaced
within that second left the new server reported dead. A client drops a call only once a
probe found the stream's current server dead, or the stream has no server
(`Iox2Client::abandon` refuses otherwise): iceoryx2 lets a server hold at most
`outstanding` requests of each client, and a client that dropped a call its live server
still held could send another, after which every receive of that server failed.

**Delivery.** A request is executed at most once, and a call gets exactly one outcome.
hypha does not retry, and it does not recognise a retried request: a retry is a new call
with a new sequence. `mayHaveRun()` and `safeToRetry()` tell the application which
outcomes allow a blind retry.

A server that must be idempotent, such as MRS `goto` sent again, is idempotent in its own
logic, as in ROS 1. Exactly-once execution would need a server-side history of call ids
that survives a restart. That is a database, not middleware.

## 6. Policy and resources

**Decision.** A service has a policy, as a topic does (cpp-api.md §7). It is a
`hypha::ServicePolicy`, named in one header that both ends include, mapped to the type by
`hypha::ServicePolicyOf<Srv>`, and stamped on the service. An opener that declares another
policy is refused with `QosMismatch`.

| Field | Default (`defaultService`) | Meaning |
|---|---|---|
| `clients` | 8, plus 3 observer slots for tools | iceoryx2 `max_clients` = clients + observers |
| `outstanding` | 2 per client | iceoryx2 `max_active_requests_per_client`. A `Calls` port asks for 1 by default |
| `requestBytes`, `responseBytes` | the encoded size of a fixed-size half, otherwise 4 KiB | the bound of each half. A larger one gives `TooLarge` |
| `timeout` | 1 s | a call's timeout, unless the client overrides it |

**Transport settings, fixed.** `apply_hypha_defaults` (`hypha_iceoryx2/src/domain.rs`)
covers publish-subscribe and events only. Left at iceoryx2's defaults, request-response
would get:

- `RetryUntilDelivered` for both backpressure strategies, which is a send that spins on
  the run thread;
- `max_nodes` 20.

Every hypha service therefore sets these explicitly:

| Setting | Value |
|---|---|
| backpressure, client and server | `DiscardData` |
| `max_servers` | 1 |
| `max_nodes` | `clients + observers + 2` |
| `max_response_buffer_size` | 1 |
| `max_borrowed_responses_per_pending_response` | 1 |
| server `max_loaned_responses_per_request` | 1 |
| `max_loaned_requests` | 1 |
| fire-and-forget, safe overflow of requests and of responses | off |

A send to a peer whose queue is full, or to no peer, returns at once. Stage 1a tests it.

**Memory.** A server's data segment holds `clients × 2·outstanding × (response buffer +
borrowed + loaned)` chunks (`static_config/request_response.rs:127-143`). With the
default policy and iceoryx2's defaults for the last three, that is 11 × 4 × 5 chunks of
4 KiB, about 880 KiB per server, or about 26 MB for 30 services. With the settings above it
is 11 × 4 × 3, about 530 KiB. The earlier estimate of 64 KiB was wrong. Section 12 gives
the measurement.

**Names** are at most 219 bytes. iceoryx2's `MAX_SERVICE_NAME_LENGTH` is 255, and the
prefix `hypha/srv/`, the key `@<16 hex>` and the event suffix `/response` take 36. A longer
name is refused at creation, and the message says 219.

**Large messages.** `nav_msgs/GetMap`, `GetPlan`, and MRS `TrajectoryReferenceSrv` or
`PathSrv` beyond about 100 points exceed 4 KiB.

- A request above the bound gives `TooLarge` from `call()`.
- A response above it gives `TooLarge` from `reply()`, and the client gets `Refused` with
  the sizes.

A port can declare a larger policy. The migration guide row says what to do instead: data
of map size goes on a latched topic, and a service carries a small request and its
acknowledgement. A 32 MB policy with the default client counts pins gigabytes.

**Idle cost.** None. A server with no calls sleeps in its request event. A client with no
call outstanding has no timeout in the `WaitSet`. A client wakes on its stream's response
event, and each woken client checks its own outstanding calls (at most `outstanding`
atomic loads), so clients of one service wake each other. A per-client event would remove
those wake-ups at the cost of one event service per client. That waits for a measurement.

**Loans.** Stage 1 copies a request or a response into its slot, as typed publishing does
today. A typed loan follows the typed loan of topics (PR #261).

## 7. Transport

**Decision: iceoryx2 request-response.** A stream uses three iceoryx2 services, and a name
one more:

| iceoryx2 service | Pattern | Who | Purpose |
|---|---|---|---|
| `hypha/srv/<name>@<hash 16 hex>` | request-response, `[u8]` payloads, the frame header as the user header of both directions | the server and its clients | the calls |
| `hypha/srv/<name>@<hash 16 hex>/request` | event | clients notify, the server listens | wakes the server |
| `hypha/srv/<name>@<hash 16 hex>/response` | event | the server notifies, clients listen | wakes the clients |
| `hypha/srv/<name>` | event, one notifier slot | the server holds the notifier | the name's server lock, across versions |

The hash is the frame's schema hash of the `.srv` MD5 (`hypha_wire::schema_hash_from_md5`),
as the topic streams of PR #266 key theirs. PR #266 is not on `develop` yet, so the key is
spelled in one place (`service/key.rs`) to be unified with it. A service name may not
contain `@`, the key's separator.

*Review of stage 1a.* A topic's bus name drops only its leading `/`, and its wake-up event
has the same name, so a topic `/hypha/srv/x` was an event `hypha/srv/x`: the server lock of
service `/x`, with a topic's eight notifier slots. Two servers of two versions served one
name that way. The `hypha/` namespace is therefore reserved, not only used: `/hypha` and
every topic name under `/hypha/` are refused with `InvalidTopic`, except `/hypha/intercept/`
where `hypha intercept` publishes. Service names follow the topic rule. The lock also
checks itself (below), so an event of its name that some other build or tool left is
refused rather than shared.

**One server per name.** The review's mechanism: server creation lists the sibling
streams `hypha/srv/<name>@*`, refuses when one has a live server, and tolerates a race of
two servers created at once, logging it at the next graph event.

*Author's note (change 7).* The rule is kept. It is enforced by the single notifier slot
of the name-level event `hypha/srv/<name>` instead, which the server holds for its
lifetime. A server opens its stream, then takes that slot, then creates its stream's server
port. iceoryx2 refuses a second notifier atomically (`ExceedsMaxSupportedNotifiers`),
whatever the version, so two servers created at once cannot both succeed, and nothing needs
logging later. The holder's notifier is named `<MD5>:<type>`, so a refusal names the type
and version the holder serves without a listing of the domain.

The lock carries the attribute `hypha.service_lock` with the service's name, and a server
opens it only with that attribute. An event of the lock's name without it was made by
something else and locks nothing: the server is refused with backend code 45 and a log
line that says so. A server port that a dead holder still holds after the lock was taken
is reclaimed the same way, within 500 ms.

**Clients and their slots.** A client of a CLI tool node (`_hyphacli_*`, as the stream's
node list names it) takes one of the three observer slots, any other client one of the
policy's; neither kind takes the other's. Both are counted after the client is created, so
two created at once cannot both take the last slot of their kind. A dead node that holds a
client and not the server is reclaimed at the next client's creation, as a dead server is
at the next server's: a client killed under a name that never returns would otherwise keep
its slot, and the requests its server holds for it, forever. A process with too few file
descriptors left for another service (about 20 for one service end) is refused with
`OutOfDescriptors`, which names the open count and the limit, rather than an iceoryx2
internal failure.

The lock is not the server's wake-up listener. A `WaitSet` takes a listener by value
(`Iox2WaitSet::attach`), so a listener that served as the lock would stay locked in a
`WaitSet` after its server was gone. Stage 1a tests the race with six processes over two
versions: exactly one wins, and the five others are refused with the holder and its type.

*Author's note (change 10).* `max_nodes` is `clients + observers + 2`, one more than the
review's formula. `hypha service list` opens a stream for an instant to count its server
and clients, which takes a node slot, the cost PR #266 found for its probes. The spare slot
keeps a listing from refusing the last client.

The alternative is two publish-subscribe streams with a correlation id, which is what all
three ports do today.

| Point | iceoryx2 request-response | Two pub/sub streams |
|---|---|---|
| Routing | A response goes to the asking client only | Every client reads every response and drops the others. Any process can answer with a matching id |
| Readiness | Counts servers | "The request topic has a subscriber", and `hypha record`, `topic echo` and Foxglove are subscribers too |
| One server | Enforced (above) | Not expressible |
| Server loss | Graceful: the call disconnects. Crash: the node probe | Nothing. The client waits for its own timeout |
| Overflow | Per-client queues that cannot overflow | A request topic can overflow, and the client never sees the loss |
| Frame header and integrity | Fits: a user header in both directions | Fits: already built |
| Lockstep | The strict cut and the two upstream edges (section 4) | A feedback loop of two topics between callback nodes holds until `--timeout` (#103) |
| Recording and replay | Not recorded; the call log is | Recorded, and replayed into live servers: the hazard |
| Discovery and tools | New: request-response services are listed for the tools | Free, but listed as topics, which they are not |
| Topic overloading | The same stream key | Free |
| Loans | Later (#261) | #261 |
| Dead-node reclaim | The same node reclaim covers client and server ports | Existing |
| Risk | An iceoryx2 API that hypha did not use before stage 1a | Mature and tested in hypha |

The topic pair has a real advantage: recording, echo, overloading and loans come free,
with machinery that hypha has already tested. Its faults are routing, readiness, server
loss and the replay hazard. None of them can be fixed above the transport, and they are
exactly the faults that make a service a control interface instead of a convenience.

**What iceoryx2 offers that stays off:** response streams, several servers per service,
fire-and-forget, safe overflow, and flatbuffer payloads. The scope is narrow on purpose.

## 8. The API

### 8.1 C++: a server in a `CycleNode` (MRS `control_manager`, `switch_tracker`)

ROS 1, abridged (`mrs_uav_managers/src/control_manager/control_manager.cpp:4223`):

```cpp
service_server_switch_tracker_ = nh_.advertiseService("switch_tracker_in", &ControlManager::callbackSwitchTracker, this);

bool ControlManager::callbackSwitchTracker(mrs_msgs::String::Request& req, mrs_msgs::String::Response& res) {
  if (!is_initialized_) {
    return false;
  }
  if (failsafe_triggered_ || eland_triggered_) {
    res.message = "can not switch tracker, eland or failsafe active";
    res.success = false;
    return true;
  }
  if (rc_goto_active_) {
    res.message = "can not switch tracker, RC joystick is active";
    res.success = false;
    return true;
  }
  auto [success, response] = switchTracker(req.value);
  res.success = success;
  res.message = response;
  return true;
}
```

hypha:

```cpp
class ControlManager final : public hypha::CycleNode {
  public:
    hypha::Requests<mrs_msgs::String> switchTrackerRequests{*this, "switch_tracker"};
    hypha::Arrivals<nav_msgs::Odometry> odometry{*this, "odometry"};
    // ... the other ports

    void setup(hypha::Node&) override {
        this->setTrigger(this->odometry);
    }

    void step(const hypha::CycleTime& time) override {
        for (hypha::Request<mrs_msgs::String>& request : this->switchTrackerRequests.requests()) {
            if (this->failsafe_triggered_ || this->eland_triggered_) {
                request.reply({.success = false, .message = "can not switch tracker, eland or failsafe active"});
                continue;
            }
            if (this->rc_goto_active_) {
                request.reply({.success = false, .message = "can not switch tracker, RC joystick is active"});
                continue;
            }
            auto [success, message] = this->switchTracker(request->value);
            request.reply({.success = success, .message = message});
        }
        this->control(time);   // sees the tracker it just switched to
    }

  private:
    std::pair<bool, std::string> switchTracker(const std::string& name);
    void control(const hypha::CycleTime& time);
};
```

```toml
[ports.servers.switch_tracker]
service = "~/switch_tracker"     # /uav1/control_manager/switch_tracker
per_step = 8
```

The switch happens at a step boundary, before the control step of the same iteration, so
it never interleaves with a step. `is_initialized_` disappears, because a step does not
run before `start()`. The requests wait for the next odometry trigger, at most one period
at 100 Hz. Stage 3 lets a `Requests` port trigger a step itself.

### 8.2 C++: a chain of calls with a deferred answer (MRS `uav_manager` takeoff)

ROS 1 (`uav_manager.cpp:1348-1590`, then `timerTakeoff`, `:843-900`). The callback checks
the state. It then switches the controller, and on failure turns the control output off
and disarms. It switches the tracker, with the same failure path. It sleeps 0.3 s, calls
the tracker's takeoff with `goal = _takeoff_height_`, and on failure calls `eland`. Then
it replies. Each step is a blocking `call()`.

Later a timer waits for the tracker to report its goal and then reach it, and switches to
the after-takeoff tracker and controller. Their results are logged, not acted on.

**With `hypha::Sequence`** (stage 1c, section 9): a coroutine in one SDK header, resumed
only by `seq.advance()` in the step, on the run thread.

```cpp
class UavManager final : public hypha::CycleNode {
  public:
    hypha::Requests<std_srvs::Trigger> takeoff{*this, "takeoff"};
    hypha::Calls<mrs_msgs::String>     switchController{*this, "switch_controller"};
    hypha::Calls<mrs_msgs::String>     switchTracker{*this, "switch_tracker"};
    hypha::Calls<mrs_msgs::Vec1>       trackerTakeoff{*this, "tracker_takeoff"};
    hypha::Calls<std_srvs::SetBool>    toggleOutput{*this, "toggle_output"};
    hypha::Calls<std_srvs::SetBool>    arming{*this, "arming"};
    hypha::Calls<std_srvs::Trigger>    eland{*this, "eland"};
    hypha::Latest<mrs_msgs::ControlManagerDiagnostics> diagnostics{*this, "control_manager_diagnostics"};

    void step(const hypha::CycleTime&) override {
        for (hypha::Request<std_srvs::Trigger>& request : this->takeoff.requests()) {
            if (auto why = this->checkTakeoff()) {
                request.reply({.success = false, .message = *why});
            } else if (this->takeoffSeq_.running() || this->afterSeq_.running()) {
                request.reply({.success = false, .message = "takeoff in progress"});
            } else {
                this->takeoffSeq_.start(request.defer(), this->runTakeoff());
            }
        }
        this->takeoffSeq_.advance();
        this->afterSeq_.advance();
    }

  private:
    hypha::Task<std_srvs::Trigger::Response> runTakeoff() {
        auto a = co_await this->switchController.call({.value = this->takeoffController_});
        if (!succeeded(a)) co_return this->disarm("could not activate '" + this->takeoffController_ + "' for takeoff", a);
        auto b = co_await this->switchTracker.call({.value = this->takeoffTracker_});
        if (!succeeded(b)) co_return this->disarm("could not activate '" + this->takeoffTracker_ + "' for takeoff", b);
        co_await hypha::sleepFor(hypha::Duration::fromMillis(300));   // upstream: the motors ramp up
        auto c = co_await this->trackerTakeoff.call({.goal = this->takeoffHeight_});
        if (!succeeded(c)) {
            this->send(this->eland, {});
            co_return failure("takeoff was not successful", c);
        }
        ++this->numberOfTakeoffs_;
        this->afterSeq_.start(this->afterTakeoff());
        co_return {.success = true, .message = "taking off"};
    }

    /// timerTakeoff(): wait until the tracker has the goal, then until it is done.
    hypha::Task<void> afterTakeoff() {
        co_await hypha::until([this] { return this->trackerHas(true); });
        co_await hypha::until([this] { return this->trackerHas(false); });
        if (auto r = co_await this->switchTracker.call({.value = this->afterTakeoffTracker_}); !succeeded(r))
            HYPHA_LOG_ERROR("could not switch to '{}' after takeoff: {}", this->afterTakeoffTracker_, hypha::describe(r));
        if (auto r = co_await this->switchController.call({.value = this->afterTakeoffController_}); !succeeded(r))
            HYPHA_LOG_ERROR("could not switch to '{}' after takeoff: {}", this->afterTakeoffController_, hypha::describe(r));
    }

    bool trackerHas(bool goal) const {
        return this->diagnostics.usable() && this->diagnostics->active_tracker == this->takeoffTracker_
            && this->diagnostics->tracker_status.have_goal == goal;
    }

    std_srvs::Trigger::Response disarm(const std::string& what, const auto& result) {
        this->send(this->toggleOutput, {.data = false});
        this->send(this->arming, {.data = false});
        return failure(what, result);
    }

    template <class Srv>
    void send(hypha::Calls<Srv>& port, const typename Srv::Request& request) {
        if (port.call(request) != hypha::CallStatus::Sent)
            HYPHA_LOG_ERROR("{}: not sent ({})", port.name(), hypha::toString(port.lastStatus()));
    }

    template <class Srv>
    static bool succeeded(const hypha::CallResult<Srv>& r) { return r.ok() && r.response().success; }

    template <class Srv>
    static std_srvs::Trigger::Response failure(const std::string& what, const hypha::CallResult<Srv>& r) {
        HYPHA_LOG_ERROR("takeoff failed: {} ({})", what, hypha::describe(r));
        return {.success = false, .message = what + " (" + hypha::describe(r) + ")"};
    }

    std::optional<std::string> checkTakeoff() const;   // the upstream checks of callbackTakeoff()

    hypha::Sequence<std_srvs::Trigger> takeoffSeq_{*this};
    hypha::Sequence<void>              afterSeq_{*this};
    std::string takeoffController_, takeoffTracker_, afterTakeoffController_, afterTakeoffTracker_;
    double      takeoffHeight_{0.0};
    uint32_t    numberOfTakeoffs_{0};
};
```

The rules of `hypha::Sequence`:

- `co_await calls.call(req)` gives the `CallResult`. A call that returns `Busy` or
  `TooLarge` resumes at once with that status as the result.
- `co_await hypha::sleepFor(d)` waits in framework time. `co_await hypha::until(pred)`
  checks `pred` at each `advance()`.
- `co_return` answers the `Deferred` given to `start()`.
- Destroying the sequence destroys its frame, and the `Deferred` then refuses.
- One heap frame is allocated per `start()`.
- Never hold a reference to step-local data, or to an input value, across a `co_await`.

**The same without the helper**, an explicit state machine. It is what stage 1c can
express before `Sequence` exists, and what `Sequence` compiles to in spirit:

```cpp
class UavManager final : public hypha::CycleNode {
  public:
    // the same ports as above

    void step(const hypha::CycleTime& time) override {
        for (hypha::Request<std_srvs::Trigger>& request : this->takeoff.requests()) {
            if (auto why = this->checkTakeoff()) {
                request.reply({.success = false, .message = *why});
            } else if (this->state_ != State::Idle) {
                request.reply({.success = false, .message = "takeoff in progress"});
            } else {
                this->pending_ = request.defer();
                this->callOrFail(this->switchController, {.value = this->takeoffController_}, State::SwitchController);
            }
        }
        this->advance(time);
    }

  private:
    enum class State {
        Idle, SwitchController, SwitchTracker, Settle, TrackerTakeoff,   // the service is open
        WaitForGoal, TakingOff, AfterTracker, AfterController            // answered; after takeoff
    };

    void advance(const hypha::CycleTime& time) {
        switch (this->state_) {
            case State::Idle:
                return;
            case State::SwitchController:
                if (auto r = this->switchController.result()) {
                    if (!succeeded(*r)) return this->disarm("could not activate '" + this->takeoffController_ + "' for takeoff", *r);
                    this->callOrFail(this->switchTracker, {.value = this->takeoffTracker_}, State::SwitchTracker);
                }
                return;
            case State::SwitchTracker:
                if (auto r = this->switchTracker.result()) {
                    if (!succeeded(*r)) return this->disarm("could not activate '" + this->takeoffTracker_ + "' for takeoff", *r);
                    this->settleUntil_ = time.now + hypha::Duration::fromMillis(300);
                    this->state_ = State::Settle;
                }
                return;
            case State::Settle:
                if (time.now >= this->settleUntil_)
                    this->callOrFail(this->trackerTakeoff, {.goal = this->takeoffHeight_}, State::TrackerTakeoff);
                return;
            case State::TrackerTakeoff:
                if (auto r = this->trackerTakeoff.result()) {
                    if (!succeeded(*r)) {
                        this->send(this->eland, {});
                        return this->finish(failure("takeoff was not successful", *r));
                    }
                    ++this->numberOfTakeoffs_;
                    this->pending_.reply({.success = true, .message = "taking off"});
                    this->state_ = State::WaitForGoal;
                }
                return;
            case State::WaitForGoal:
                if (this->trackerHas(true)) this->state_ = State::TakingOff;
                return;
            case State::TakingOff:
                if (this->trackerHas(false))
                    this->callOrLog(this->switchTracker, {.value = this->afterTakeoffTracker_}, State::AfterTracker);
                return;
            case State::AfterTracker:
                if (auto r = this->switchTracker.result()) {
                    if (!succeeded(*r)) HYPHA_LOG_ERROR("could not switch to '{}' after takeoff: {}", this->afterTakeoffTracker_, hypha::describe(*r));
                    this->callOrLog(this->switchController, {.value = this->afterTakeoffController_}, State::AfterController);
                }
                return;
            case State::AfterController:
                if (auto r = this->switchController.result()) {
                    if (!succeeded(*r)) HYPHA_LOG_ERROR("could not switch to '{}' after takeoff: {}", this->afterTakeoffController_, hypha::describe(*r));
                    this->state_ = State::Idle;
                }
                return;
        }
    }

    /// A call of the open sequence: a call that is not sent ends it with a refusal.
    template <class Srv>
    void callOrFail(hypha::Calls<Srv>& port, const typename Srv::Request& request, State next) {
        if (const hypha::CallStatus status = port.call(request); status != hypha::CallStatus::Sent) {
            this->send(this->toggleOutput, {.data = false});
            this->send(this->arming, {.data = false});
            return this->finish({.success = false, .message = std::string(port.name()) + ": " + hypha::toString(status)});
        }
        this->state_ = next;
    }

    /// A call after the answer: a call that is not sent is logged and the sequence ends.
    template <class Srv>
    void callOrLog(hypha::Calls<Srv>& port, const typename Srv::Request& request, State next) {
        if (const hypha::CallStatus status = port.call(request); status != hypha::CallStatus::Sent) {
            HYPHA_LOG_ERROR("{} after takeoff: {}", port.name(), hypha::toString(status));
            this->state_ = State::Idle;
            return;
        }
        this->state_ = next;
    }

    template <class Srv>
    void disarm(const std::string& what, const hypha::CallResult<Srv>& r) {
        this->send(this->toggleOutput, {.data = false});
        this->send(this->arming, {.data = false});
        this->finish(failure(what, r));
    }

    void finish(const std_srvs::Trigger::Response& response) {
        this->pending_.reply(response);
        this->state_ = State::Idle;
    }

    // send(), succeeded(), failure(), trackerHas(), checkTakeoff() as above

    State                              state_{State::Idle};
    hypha::Deferred<std_srvs::Trigger> pending_;
    hypha::Time                        settleUntil_{};
    // names, height, counter as above
};
```

```toml
[ports.servers.takeoff]
service = "~/takeoff"
[ports.clients.switch_controller]
service = "/uav1/control_manager/switch_controller"
[ports.clients.switch_tracker]
service = "/uav1/control_manager/switch_tracker"
[ports.clients.tracker_takeoff]
service = "/uav1/control_manager/landoff_tracker/takeoff"
timeout_ms = 2000                 # overrides the policy's 1 s
[ports.clients.toggle_output]
service = "/uav1/control_manager/toggle_output"
[ports.clients.arming]
service = "/uav1/hw_api/arming"
[ports.clients.eland]
service = "/uav1/control_manager/eland"
```

**Against the current port** (`examples/mrs/managers/uav_manager.cpp`, 392 lines), which
already implements this full sequence by hand, the hand-written form above drops:

- `callStep()` and the `sent_` flag: `result()` is empty until the outcome arrives;
- the hand-made timeout: every failure is an outcome;
- the ticket bookkeeping: `Deferred` replaces it.

It keeps one state per call. The `Sequence` form keeps the order of the upstream callback.

Two corrections to the current port, which the example follows upstream on:

- A failed takeoff call calls `eland`. The current port turns the output off and disarms
  instead, and declares no `eland` client.
- The after-takeoff switches belong to the sequence after the answer, not to the service.

### 8.3 C++: a setter in an `ApplicationNode` (`arena_camera`, `set_exposure`)

ROS 1 (`arena_camera_node.cpp:69,1434`):

```cpp
set_exposure_srv_(nh_.advertiseService("set_exposure", &ArenaCameraNode::setExposureCallback, this))

bool ArenaCameraNode::setExposureCallback(camera_control_msgs::SetExposure::Request& req,
                                          camera_control_msgs::SetExposure::Response& res) {
  res.success = setExposure(req.target_exposure, res.reached_exposure);
  return true;
}
```

hypha. The worker thread owns the device (the port's `std::jthread worker_`). The handler
defers the request and hands it to the worker, which answers between two grabs. No lock
guards the device.

```cpp
void ArenaCameraNode::setup(hypha::Node& node) {
    this->setExposure_ = node.advertiseService<camera_control_msgs::SetExposure>(
        "set_exposure", [this](hypha::Request<camera_control_msgs::SetExposure>& request) {
            if (!this->commands_.tryPush(ExposureCommand{request->target_exposure, request.defer()})) {
                // the deferred handle in the rejected command refuses when it is destroyed
                HYPHA_LOG_WARN("set_exposure: command queue full");
            }
        });
}

// on the worker, between two GetImage calls
while (auto command = this->commands_.tryPop()) {
    float reached = 0.0F;
    const bool success = this->device_.setExposure(command->target, reached);
    command->reply.reply({.reached_exposure = reached, .success = success});   // any thread
}
```

```toml
[ports.servers.set_exposure]
service = "~/set_exposure"
```

`[[nodiscard]]` on `advertiseService` keeps the handle from being dropped by mistake.
`commands_` is the application's own bounded queue. A port that keeps upstream's shape can
instead answer in the handler under a mutex: `request.reply({...})`.

### 8.4 `std_srvs`

The SDK ships `std_srvs` (`Empty`, `Trigger`, `SetBool`) as it ships `std_msgs`. Today
it ships no `.srv` type, and the MRS example declares its own `std_srvs`
(`examples/mrs/msg/std_srvs/srv/`). Designated initializers keep the calls short:

```cpp
request.reply({.success = true, .message = "armed"});   // Trigger, SetBool
request.refuse("not armed");
(void)arming.call({.data = true});                      // SetBool; the status says Sent, Busy or TooLarge
(void)reset.call({});                                   // Empty, Trigger
```

A porter who writes `client.call(srv)` with a ROS-style `Srv` object hits a deleted
overload. Its `static_assert` names the guide row: `call(Srv&) does not exist: pass
srv.request, and read the result in a later step (ros1-migration-guide.md, services)`.

### 8.5 A client in a plain node, and in a tool or a test

```cpp
[[nodiscard]] auto trigger = node.serviceClient<std_srvs::SetBool>(hypha::Service{"/localization/ekf/trigger_node"});
if (trigger.call({.data = true}, [](const hypha::CallResult<std_srvs::SetBool>& r) {
        if (!r.ok() || !r.response().success) HYPHA_LOG_WARN("trigger_node: {}", hypha::describe(r));
    }) != hypha::CallStatus::Sent) {
    HYPHA_LOG_WARN("trigger_node: not sent");
}

// a gtest, a tool or a worker thread: no WaitSet runs on this thread
auto takeoff = node.serviceClient<std_srvs::Trigger>(hypha::Service{"/uav1/uav_manager/takeoff"}, hypha::blocking);
if (!takeoff.waitForServer(hypha::Duration::fromSeconds(5))) { /* no server */ }
const hypha::CallResult<std_srvs::Trigger> r = takeoff.callAndWait({});
```

### 8.6 Tables

| Table | Keys | Meaning |
|---|---|---|
| `[ports.servers.<port>]` | `service` | The name. With `optionalService`, `service = ""` disables the port, as `optionalTopic` does for a topic port |
| | `per_step` | Requests a step takes, default 8, at most `clients × outstanding`. The rest wait for a later step |
| `[ports.clients.<port>]` | `service` | The name |
| | `timeout_ms` | Overrides the policy's timeout |
| | `outstanding` | Calls in flight, default 1 for a `Calls` port, at most the policy's |

A missing table for a port that requires one is refused with exit 78. The error prints
the stanza to paste:

```text
hypha: server port 'switch_tracker' has no table; add to the parameter file:
[ports.servers.switch_tracker]
service = "~/switch_tracker"
```

The tables also give a plain node's `advertiseService("set_exposure", ...)` its name, which
closes the migration guide's gap "a service name does not resolve from the ports tables".

### 8.7 C ABI (sketch; sizes, ownership and null rules beside each declaration)

```c
HyphaStatus hypha_service_server_create(HyphaNode*, const char* name, size_t name_len,
                                        const HyphaServiceSpec* spec, HyphaServer** out);
HyphaStatus hypha_service_client_create(HyphaNode*, const char* name, size_t name_len,
                                        const HyphaServiceSpec* spec, uint8_t blocking,
                                        HyphaClient** out);
/* spec: type name, srv MD5, both halves' definitions and MD5s, policy (timeout included). */
HyphaStatus hypha_client_ready(const HyphaClient*, uint8_t* out);
HyphaStatus hypha_client_pending(const HyphaClient*, uint8_t* out);
HyphaStatus hypha_client_call(HyphaClient*, const uint8_t* request, size_t len,
                              HyphaCallStatus* status, uint64_t* out_call);
HyphaStatus hypha_client_call_and_wait(HyphaClient*, const uint8_t* request, size_t len,
                                       HyphaCallResult* result, uint8_t* response,
                                       size_t capacity, size_t* response_len);
HyphaStatus hypha_client_wait_for_server(HyphaClient*, int64_t timeout_nanos, uint8_t* out);
/* Both: InvalidState on a client that is not blocking, or on a thread inside a dispatch. */
HyphaStatus hypha_waitset_attach_client(HyphaWaitSet*, HyphaClient*, HyphaCallDone cb,
                                        void* context, void (*release)(void*));
HyphaStatus hypha_waitset_attach_server(HyphaWaitSet*, HyphaServer*, HyphaServe cb,
                                        void* context, void (*release)(void*));
/* HyphaServe receives a HyphaRequestRef: borrowed, valid during the callback only. */
HyphaStatus hypha_cycle_bind_server(HyphaCycle*, HyphaServer*, uint32_t per_step, uint32_t* out_binding);
HyphaStatus hypha_cycle_bind_client(HyphaCycle*, HyphaClient*, uint32_t* out_binding);
HyphaStatus hypha_cycle_image_request(const HyphaCycleImage*, uint32_t binding, size_t index,
                                      HyphaRequestRef* out);       /* borrowed, valid in the step */
HyphaStatus hypha_cycle_image_call_result(const HyphaCycleImage*, uint32_t binding, size_t index,
                                          HyphaCallResult* out);
HyphaStatus hypha_cycle_emission_call(HyphaCycleEmission*, uint32_t binding, const uint8_t* request,
                                      size_t len, HyphaCallStatus* status, uint64_t* out_call);
HyphaStatus hypha_request_reply(HyphaRequestRef, const uint8_t* response, size_t len);
HyphaStatus hypha_request_refuse(HyphaRequestRef, const char* reason, size_t reason_len);
HyphaStatus hypha_request_defer(HyphaRequestRef, HyphaDeferred** out);   /* owned */
HyphaStatus hypha_deferred_reply(HyphaDeferred*, const uint8_t* response, size_t len); /* any thread; consumes */
HyphaStatus hypha_deferred_refuse(HyphaDeferred*, const char* reason, size_t reason_len); /* any thread; consumes */
void        hypha_deferred_destroy(HyphaDeferred*);              /* refuses an unanswered one */
```

- `HyphaCallStatus`: `Sent`, `Busy`, `TooLarge`.
- `HyphaCallOutcome`: `Answered`, `Refused`, `NoServer`, `NotSent`, `ServerLost`, `Timeout`.
  These are outcomes, not statuses, so `HyphaStatus` 19 stays retired (as the issue says).
- A reply to a slot whose server or client is gone returns `Disconnected`.
- `abi.revision` goes up by one.

*Stage 1b, as built.* The plain-node half of this sketch is coarser: ten functions
(interface-specification.md, Services). A server's create attaches it to the node's
`WaitSet` (no separate `hypha_waitset_attach_server`); a request is answered by one
`hypha_request_answer` with a kind, reply or refuse, whether the handler's or a deferred
one, and a deferred request is the same handle type, released by `hypha_request_destroy`;
a client is one create whose `blocking` field decides whether its one
`hypha_client_call` delivers the outcome at a later turn or before it returns, so
`hypha_client_call_and_wait` and its response buffer, `hypha_waitset_attach_client` and
a per-client callback go; the outcome reaches a per-call callback with its description
and its `may_have_run`/`safe_to_retry` already decided; `ready` and `pending` are one
`hypha_client_status`. A context's `release` runs exactly once, also on a refusal. The
cycle functions stay for stage 1c.

### 8.8 Rust SDK

The tools need a blocking client only:
`hypha_node::sdk::ServiceClient::call_and_wait(&self, request: &[u8], timeout) ->
Result<CallResult, ServiceError>`. It is typed by the schema it reads from the store, as
`hypha topic echo` reads a topic's. AGENTS.md lets the SDK lag a new capability, and a
Rust server is not needed.

## 9. Tools and test support

**`hypha service` (stage 1b, because operators call the camera setters):**

- `hypha service list` prints one line per stream: the name, type, MD5, server node,
  client count, and the calls answered, refused and timed out.
- `hypha service info <s>` adds the definition and the policy.
- `hypha service call <s> '{data: true}'` reads the request as YAML or JSON and encodes it
  from the definition in the schema store. It calls with `--timeout` (by default the
  policy's) and prints the response as YAML, decoded as `hypha topic echo` decodes a
  message. It exits non-zero and prints the outcome for anything but `Answered`.
- `hypha service call <s>` with no body prints the request template.
- When the server is on simulated time and the clock is paused, the tool says so:
  `server '<n>' is on simulated time; /clock paused at <t>`.
- No tool encodes a message from text today, so the encoder is new work.
- The tool's node is a CLI node, so its client takes an observer slot, never a declared
  client's slot.

**Security.** No new mechanism: the domain and the `/dev/shm` permissions are the
boundary. The server logs every call from a `_hyphacli_` node at INFO with its pid, and
the call log records it.

**Introspection.** `~/introspection` lists a node's servers and clients with their
policies (FSR-41). `~/state` counts per endpoint:

- per server: received, answered, refused, unanswered, `abandoned`, deferred now;
- per client: each outcome, `results_unread` and `late_responses`.

`hypha top` shows a services panel from these and from the call log.

**`StepHarness`** (stage 1c):

- `harness.request(port, request)` delivers a request to the next step. It returns a
  probe that holds the reply, the refusal or the deferral after the step.
- `harness.calls(port)` lists what the step called.
- `harness.answer(port, response)` or `harness.outcome(port, hypha::CallOutcome::ServerLost)`
  gives the next step its result.

A client under test therefore needs no fake server process, and every failure branch of
the `uav_manager` sequence becomes a step test.

**`hypha::Sequence`** (stage 1c, with `Calls`): about 200 lines of C++23 coroutine in one
SDK header, with no C ABI. Its rules are in section 8.2. In a plain node it is resumed
from an `onResult` callback.

**The ROS bridge** (stage 5). For services, the direction ports need first is ROS 1 to
hypha: a ROS operator or tool (`rosservice call /uav1/uav_manager/takeoff`) calls a
hypha server. The bridge advertises the service on the ROS master and forwards each call,
keeping one hypha client per ROS service.

## 10. Migration guide rows (replace the stop-and-ask)

| ROS 1 | hypha |
|---|---|
| `nh.advertiseService(name, &C::cb, this)` in a cycle node | A `hypha::Requests<Srv>` port. In the step, `for (auto& r : port.requests()) r.reply({...});`. Name it in `[ports.servers.<port>] service = ...` |
| `nh.advertiseService` in a plain or application node | `[[nodiscard]] auto s = node.advertiseService<Srv>("<port>", [](hypha::Request<Srv>& r) { r.reply({...}); });` |
| callback `return false` | `r.refuse()` or `r.refuse("why")` |
| a callback that works over time, makes nested calls or sleeps | `r.defer()`, then answer the `hypha::Deferred<Srv>` in the step that finishes. Each nested call is a `Calls` port. Use `hypha::Sequence` for a chain (§8.2) |
| `nh.serviceClient<Srv>(name)` | A `hypha::Calls<Srv>` port in a cycle, or `node.serviceClient<Srv>("<port>")` in a plain node. The timeout comes from the policy (1 s), or `timeout_ms` in `[ports.clients.<port>]` |
| `client.call(srv)` in a callback | In a cycle: `port.call(srv.request)` in one step, then `port.result()` in a later one. In a plain node: `client.call(request, onResult)`. A blocking call on the run thread is refused (`InvalidState`) |
| `client.call(srv)` in a worker thread, a test or a tool | `node.serviceClient<Srv>(name, hypha::blocking)`, then `callAndWait(request)` |
| `ros::service::waitForExistence(name, t)`, `client.waitForExistence(t)` | In a node: nothing. A call waits for its server until its timeout. In a test or a tool: `blockingClient.waitForServer(t)` |
| `client.exists()` | `client.exists()` (`ready()`) |
| `ros::service::call(name, srv)` | A client, as above. Only tools and tests call without one |
| `serviceClient(name, true)` (persistent) | The default. Every client stays connected |
| a service called from inside a callback | A state over steps, or `hypha::Sequence` (§8.2), not a nested wait |
| a service whose response is a map, a path or a cloud | The data on a latched topic. The service carries a small request and its acknowledgement (§6, large messages) |
| `std_srvs/*` | Shipped by the SDK |
| `rosservice call` | `hypha service call` |

The two rows that point at `cpp-api.md` §8 (request and reply topics with an identifier)
go, and so does the stop-and-ask item "A service whose caller blocks on the reply".

## 11. Staging

Sizes are rough estimates of changed lines including tests, made by comparison with the
cycle `Arrivals` path and the topic overloading PR. They are not measured.

| Stage | Content | Tests | Size |
|---|---|---|---|
| **1a Transport** (done) | `hypha_iceoryx2`: server and client ports with the settings of §6; the name lock, and the reclaim of a dead holder; the stream key; sealing with the role codes; answer kinds; the server's slot table with handles; `ServicePolicy` with its timeout; the 219-byte limit; the listing for the tools; the probe with its rate | the failure table in two processes, creation races, many clients, the measurements of §12 | 2,000 |
| **1b Runtime, plain nodes, C ABI, C++, tools** | `hypha_node`: calls waiting for a server, timeouts in the `WaitSet`, outcomes and `CallStatus`, the dispatch-thread refusal, blocking clients; C ABI; `advertiseService`, `serviceClient`, `Request`, `Deferred`, `CallResult`; `std_srvs` in the SDK; tables; the call log; `hypha service list\|info\|call`; docs (`interface-specification.md`, `cpp-api.md` §8, `features.md`, `execution-model.md`, `communication-integrity.md`) | the failure table through C++; a `Deferred` answered from another thread, destroyed unanswered, and outliving its server; a call before its server; installed-SDK flow; a CLI test of `service call` | 3,000 |
| **1c Cycle ports and lockstep** | First task, the lockstep tests: two identical runs; `--chunk 50` against `--chunk 200`; with and without `hypha topic echo`; `--loop`; a triggered client against a triggered server; a chain of deferred replies across three nodes; a client and its server in one `WaitSet`. Then `Requests<Srv>`, `Calls<Srv>`, answers and calls sent after the step, `NotSent`, requests a step never read staying queued, the strict cut and the two upstream edges, the §6.6 amendment, `StepHarness`, `hypha::Sequence` | the lockstep list; harness tests; `Sequence` tests | 2,500 |
| **1d The first users** | the MRS host and the rclcpp shim move onto it; `mrs_bridge/Call` and `ServiceCall` are deleted; the EKF trigger starts on bus traffic | the five MRS scenarios; the Autoware EKF trigger | 1,500, of which about 600 deleted |
| 2 Introspection | `~/state` and `~/introspection` rows, the `hypha top` panel | | 600 |
| 3 Requests as a trigger | a `Requests` port can trigger a cycle, with no silence limit and no fallback | the trigger rules of execution-model.md §6.6 for a request | 500 |
| 4 Replay of the call log | `hypha play --serve`, `--call` | a recorded call served back to a client under test; recorded calls made into a server | 1,200 |
| 5 ROS bridge | ROS 1 to hypha for services | a `rosservice call` reaches a hypha server in the bridge's container test | 1,000 |
| 6 Large payloads | typed loans for requests and responses (after #261); a per-client wake event, if measured to matter | | 500 |

The camera needs stages 1a and 1b. The flight needs 1a to 1c. Stage 1d is the first use,
and it deletes the two hand-made wire formats (`features.md` rule 4: no second path). Each
stage is a pull request of its own. Stage 1b follows a review of 1a.

**Requirements.** New entries, for the owner:

- **FSR-42** The software shall provide request and response between nodes, typed by a ROS 1
  `.srv` definition. A service name shall have at most one server across its versions, and
  a second server shall be refused at creation. Each call shall end in exactly one outcome,
  and the outcomes shall distinguish answered, refused, no server, not sent, server lost
  and timeout. No request shall be lost without an outcome.
- **FSR-43** A call shall carry a timeout in framework time. A call that waits for its
  outcome shall be refused on a thread that runs a dispatch.
- **FSR-44** A request made by a replay participant shall be served at a deterministic step
  of its server. A replay with services between participants shall repeat exactly,
  whatever `--chunk` is and whatever observers are attached.

Existing requirements that widen:

- **FSR-26** gains "... or a server of a service whose server slot it holds starts".
- **FSR-09/10:** service frames and the role codes.
- **FSR-13:** a request or a response wakes its reader.
- **FSR-22/41:** services in the tools and in the snapshot.
- **FSR-24:** the request-response services are not recorded; the call log is.
- **NFR-05:** the service bounds.

FSR-21 already covers `.srv` generation.

## 12. Evidence: the spike, then stage 1a

**The spike** (removed; its tests are now `crates/hypha_iceoryx2/tests/service_transport.rs`)
established, on macOS (aarch64) with iceoryx2 0.10.1, the following:

- The frame header fits as the user header of both directions.
- The server sleeps in its request event until a call comes.
- A second server is refused.
- A call with no server has 0 connections at send.
- A server killed with SIGKILL while it holds a request stays connected and keeps its slot
  until its dead node is reclaimed. The service's node list reports that node dead 1.4 to
  3.7 ms after the kill.
- `ActiveRequest` is `Send`.
- A round trip of a `SetBool` call took a median of 51 to 160 µs and a p99 of 1.3 to
  4.8 ms, at load average 51 to 53.

**Stage 1a** (`crates/hypha_iceoryx2/tests/service_transport.rs`, `--no-capture` prints
these):

| Measurement | macOS (aarch64), host | Linux 7.0 (linuxkit, aarch64), builder container |
|---|---|---|
| Round trip, 2,000 `SetBool` calls between two processes | median 53 to 92 µs, p99 0.19 to 15.9 ms, across six runs at load average 29 to 34 | median 268 µs, p99 3.1 ms, max 32.9 ms, at load average 8.3 |
| Killed server found dead by the probe | 2.8 ms after the kill; one probe 1.0 ms at load 35; a probe within its second 2 µs | 2.7 ms after the kill; one probe 1.3 ms |
| Memory of a server under `defaultService` | not measurable: no `/dev/shm` | 549 KiB with its stream and events, against 528 KiB for the formula of §6 (132 chunks of 4 KiB) |
| Memory of a client | not measurable | 24 KiB |
| Descriptors | server 18, client 20 | server 13, client 14 |

The macOS spread is the loaded host (load average 29 to 71 during the work), not the
transport. On Linux, a container under Docker's VM, these numbers say that the transport
works there; they are not a benchmark of the RT target.

## 13. Not in scope, and why

- **Parameters as services** (`get_param`, `set_param`, `dynamic_reconfigure`).
  Parameters are an immutable description read once (FSR-18), and a runtime change through
  a service would be a second configuration path. A node that needs a runtime setting
  serves a setter of its own, as the camera does.
- **Actions.** An action is a goal, feedback, a result and a cancel. In hypha it would be a
  goal service with a deferred reply (the result), a feedback topic and a cancel service,
  all built on these primitives. It needs a goal id across them and a rule for a preempted
  goal. That is a library above the core, worth writing once a port needs one; the
  camera's two grab actions are dropped today.
- **Response streams, several servers, fire-and-forget.** iceoryx2 offers them. No port
  needs them, and each would weaken "exactly one outcome from the one server".
- **A blocking call inside a run.** Section 3.
- **Service wildcards.** A call is addressed to one server, so a pattern has no meaning
  there. This is already the rule in interface-specification.md: a wildcard never follows
  a request-response service.
- **Exactly-once execution.** Section 5.

## 14. Open risks

- **The maturity of iceoryx2's request-response in 0.10.1.** Stage 1a tests creation
  races, crashes, many clients and bounds in two processes, on macOS and on Linux in the
  builder container. They have not run on the RT target. A vendored iceoryx2 (open PR, for
  another reason) would let a fix land without waiting for a release.
- **A killed client.** A client that exits normally frees its held requests at the server's
  next turn (`drop_abandoned`). A killed client stays connected, as a killed server does,
  until its node is reclaimed, so its requests keep their slots until then. Stage 1a does
  not test that case.
- **The liveness probe's cost.** 0.55 to 1.8 ms per probe at load 51, on the run thread,
  at most once per service per second.
- **Lockstep.** The strict cut is the rule every input already follows. The two upstream
  edges and the §6.6 amendment are new code in stage 1c, and the first task of 1c tests
  them.
- **Shared response event.** The clients of one service wake each other. That is harmless
  at 8 clients, and a service with many clients should be measured before relying on it.
- **Naming.** `Requests`, `Calls`, `Request`, `Deferred`, `Sequence`, `advertiseService` and
  `serviceClient` are proposals. The ROS 1 names are kept where a porter types them.
