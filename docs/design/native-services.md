# Native services

Design for #132: typed request and response between nodes, from a ROS 1 `.srv`. Status:
proposal, with a spike that checks its transport assumptions
(`crates/hypha_iceoryx2/tests/service_spike.rs`). Nothing here is implemented. Each
decision gives its reason. Where it differs from the proposal in the issue, it says so.

Readers: the owner, who decides; whoever implements the stages of section 11.

## 0. Why now, and what the design must serve

Every port that needs a call builds one out of two topics, and each builds a different
one (issue #132): `mrs_bridge/Call` in `examples/mrs/host/include/mrs_host/service.h`,
`hypha_rclcpp_msgs/ServiceCall` in the Autoware shim, and a `request_id` field added to
`nav_msgs/GetMap` in the `map_server` port, which changes its MD5. The vetting reports
in the playground show where services stop a port:

| Port | Services | Without them |
|---|---|---|
| MRS UAV system | 10 servers (4 `control_manager`, 3 `LandoffTracker`, 1 `uav_manager`, 2 simulator) and 19 clients (mission 9, teleop 5, `uav_manager` 5). Arming, offboard, `toggle_output`, `switch_controller`, `switch_tracker`, takeoff, `goto` | The flight. They are the control interface of the system. |
| Autoware | the EKF's `trigger_node_srv`: the node publishes nothing until it is called with `true` | The estimate. `gnss_pose_initializer` is a workaround. |
| `arena_camera` | seven setters (`set_exposure`, `set_gain`, ...) and `set_camera_info` | Dropped from the port. |
| `map_server`, `diagnostic_aggregator` | `static_map`, `change_map` | Waiting, or a third wire format. |

Two shapes cover all of it. A **setter or a switch**: the handler answers at once
(`switch_tracker`, `set_exposure`, `arming`, `trigger_node`). A **sequence**: the handler
starts work, calls other services, and answers when done (`uav_manager` takeoff: switch
the controller, switch the tracker, wait 0.3 s, call the tracker's takeoff, answer).
ROS 1 runs the sequence inside the callback, blocking a spinner thread on each nested
`call()` and on `ros::Duration(0.3).sleep()`
(`mrs_uav_managers/src/uav_manager.cpp:1480-1560`).

## 1. What a service is

**Decision.** A service is a name and a `.srv` type. A client sends one request and
gets exactly one outcome. A server answers each request once. A name resolves as a topic
name does (absolute, relative to the node's namespace, `~/` private), from a port table
or a literal. Within one service, a stream is identified by its name plus the MD5 of the
`.srv` type, as a topic stream is identified by its name plus the message MD5
(PR #266). Each stream has at most one server.

*Reason.* A ROS 1 porter expects one name and one type. The `.srv` MD5 already exists:
`hypha_msggen` emits `Request`, `Response` and the service type with ROS 1's service MD5
over both halves (`hypha_msgdef::resolve::service_identity`). FSR-21 already requires the
generation. Using the same stream key as topics gives a client and a server of two
versions of one `.srv` the behaviour that topics get during a migration (each version is
its own stream, and neither fails). Users then learn one naming rule, not two.

| Situation | Behaviour |
|---|---|
| A second server for a stream | Its creation fails with `ResourceLimit`. The message names the holder: `hypha: service '/uav1/arming' (std_srvs/SetBool 09fb0352): already served by node 'hw_api' (pid 4711)`. Spike: iceoryx2 refuses it (`ExceedsMaxSupportedServers`). |
| The holder is a dead node | The same refusal, which names the node as dead and says how to free the slot: the node restarts under its name, and its reclaim frees the slot, or `hypha clean` runs. A server never takes a dead node's slot itself, because FSR-26 gives a dead node's resources to its own restart. Spike: the dead server keeps the slot until its node is reclaimed, and then a new server is created. |
| A client before its server | Not an error (FSR-25: start order must not matter). `ready()` says whether a live server of the client's stream exists now. A call made with no server ends `NoServer`. There is no `waitForExistence`. Section 10 says what a porter writes instead. |
| A server of another version only | `NoServer`. The client logs once, and again when the situation changes: `hypha: service '<s>' has a server, not of type <T> (<md5>): served is <T> (<md5>)`, the topic warning of PR #266 in service form. |
| A server that exits normally mid-call | The outstanding calls end `ServerLost` on the client's next turn. The server's teardown wakes its clients. |
| A server that crashes mid-call | Section 5: `ServerLost` at the call's deadline. |

Two servers of one stream would need a rule for which one answers, and ROS 1 has none,
so they are refused.

## 2. The server's execution model

**Decision.** Option (c) of the brief: one mechanism with two front ends.

- **Underneath** the node's `WaitSet` holds the server's queue of received requests. A
  request event wakes the run. No timer and no poll is involved (spike: the server process
  sleeps in `timed_wait` of the request event and wakes within the round trip below).
- **A cycle** reads requests as an input port, `hypha::Requests<Srv>`. They are queued
  arrivals with the cycle's capture, budget and replay rules. The step answers through the
  handle it is given.
- **A plain node or `ApplicationNode`** registers a callback,
  `node.advertiseService<Srv>(name, handler)`, which runs on the `WaitSet` thread between
  turns, as a subscription callback does.

*Reason.* The MRS managers are cycle nodes. `control_manager` is a triggered cycle, and
its four servers already answer inside its step, through `Arrivals` of request topics
(`examples/mrs/managers/control_manager.cpp:429-470`). A server that runs beside the step
would bring back the concurrency that cycles exist to remove: a `switch_tracker` callback
that changes the active tracker between two steps that read it. A camera or a map server
is a plain or application node with no cycle, and a callback is the natural form there.
One queue underneath keeps the integrity, the limits and the counters in one place in
Rust (FSR-19). The issue made the cycle port a follow-up. This design puts it in the first
version, because the flight is a cycle-node system.

**Replies.** A handler gets a `hypha::Request<Srv>&` and does one of three things with it:

- `reply(response)` answers it.
- `refuse()` gives the client the outcome `Refused`, as a ROS 1 callback does when it
  returns `false`.
- `defer()` returns a `hypha::Deferred<Srv>`. This is a move-only handle that the node
  keeps and answers in a later step or callback. Its destructor refuses the request if it
  was not answered. It may be answered from another thread, which the spike's
  `an_active_request_and_a_pending_response_are_send` confirms is possible: iceoryx2's
  `ActiveRequest` is `Send` on the thread-safe service.

A request that leaves its handler with none of the three is refused, counted
(`service.unanswered`) and logged once. It is never left pending without an owner.
Deferred replies are in scope now: `uav_manager` takeoff needs them, `mrs::Serve` already
offers them (its handler returns `std::nullopt` and later calls `respond(ticket, ...)`),
and Autoware's `component_interface_utils` handlers need them (issue). The number a
server may hold deferred is bounded (section 6).

**In a step, replies leave after the step**, in reply order with its outputs. A step that
fails sends none of its outputs (FSR-38), and its captured requests then end `Refused`
with the reason `server step failed`. The client always gets an outcome, and a failed step
has no visible effect. A deferred handle that is answered in a later step follows that
step's rule.

**Not chosen.**

- A handler that returns the response (`Response handle(const Request&)`). It cannot
  defer, so a second form would be needed for the sequence shape. One handle form serves
  both, and `request.reply({...})` is as short.
- Long-running work in a service. ROS 1 guidance says to use an action for it. A
  deferred reply answers when a short sequence ends. It does not stream progress.

## 3. The client's execution model

**Decision.** Inside a run, a call is asynchronous, and its outcome arrives later:

- **In a cycle** a `hypha::Calls<Srv>` port is both the output and the input of calls.
  `call(request)` in a step stages the request. It is sent after the step succeeds, like
  an output, so a failed step sends no request. Its outcome is an input of a later step:
  `calls.result()` (or `results()` when more than one may be outstanding).
- **In a plain node** `client.call(request, onResult)` sends at once. `onResult` runs on
  the `WaitSet` thread at a later turn, never inside `call()`.

There is one rule for both. `call()` refuses only what it can know locally: `Busy` when
the client already has its limit of outstanding calls, and `TooLarge` when the request is
above the service's bound. Everything else is an outcome, delivered exactly once, later:
`Answered`, `Refused`, `NoServer`, `ServerLost`, `DeadlineExpired`, `InvalidResponse`.
The issue has `send` return `NotAvailable` when there is no server. In a step the send
happens after the step, so `NoServer` cannot be a return value there. Making it an outcome
everywhere keeps one rule.

**The deadline** is required. It is in the client's framework time and is given in code
(`hypha::timeout(d)`) or in its table (`timeout_ms`). A client without one is refused at
start, and the message names the key. There is no wait without an end. In framework time,
a replay expires the same deadlines on every run.

**A blocking call** (`callAndWait(request)`) exists only for a client on a thread that
runs no `WaitSet`: a tool, a test, or a worker thread of an `ApplicationNode`. The wait is
on the caller's thread and uses the client's own response event. The library starts no
thread, so FSR-14 holds. On a thread that is running a `WaitSet` dispatch it is refused
with `InvalidState`, and the message says what to write:
`hypha: callAndWait on the run thread would wait for the thread that must deliver its
answer; use a Calls port in a cycle or call(request, onResult) in a callback`. The test is
"is this thread inside a dispatch". It is done at run time because C++ cannot see it at
compile time. Schema lookup inside a callback already applies the same rule
(execution-model.md §5.1).

*Reason.* A blocking call in a step stalls every other input of the node. With a server in
the same `WaitSet` it can never be answered, and under lockstep it holds the replay until
`--timeout` (`replay-flow-control.md` §6.7). The common ROS 1 pattern is "in a callback,
call another node's service and use the result". It becomes a state over steps in a
cycle, or a continuation in a plain node. Section 8 shows the `uav_manager` takeoff
written this way. It comes out shorter than the hand-written `callStep` state machine of
the current port.

## 4. Time, lockstep, recording, replay

**Stamps.** A request carries the client's framework send time in its frame header
(`framework_sent_nanos`), as a topic frame does. For a staged call that is the step's
time. A response carries the server's time when it was answered.

**Lockstep (decision, as the issue proposes).** The response edge is an ordinary upstream
edge, so a participant server is an upstream of its participant clients. The request edge
does not hold the server. It is the loop's delay element:

- The server admits requests at `/clock` sample boundaries. When it reads the sample
  `C(k)`, it admits every request stamped at or before the previous sample's value
  `C(k-1)`, in the order (stamp, client id, sequence). The steps of that sample see them.
- This is deterministic. A participant sends its data before its progress report
  (`replay-flow-control.md` §4.2). The player sends `C(k)` only after every participant
  acknowledged `C(k-1)`. So every request stamped at or before `C(k-1)` is in the queue
  when `C(k)` is read, and none stamped later can be admitted.
- No loop holds. The client waits for the server's progress. The server waits for no
  client. A response is stamped later than the request it answers.
- The cost: under replay a call takes one clock sample (one `--chunk`) longer than it does
  live. A deadline shorter than one chunk expires. The docs must say so.

A server that is a plain callback node takes no part, as for topics
(execution-model.md §11.2). Its responses are stamped when they are sent, and a replay
with it is not deterministic. That is the existing rule for plain callbacks.

**Recording (decision).** The first version records no service traffic. `hypha record`
follows publish-subscribe services only, so a request-response service is invisible to it
by construction. A recording can therefore never call a live server. That fixes the
replay hazard of the topic pair: a recorded `trigger_node` request started the live EKF
again. In stage 4 a server can publish what it served on an observer stream
`<service>/calls` (request, response, outcome, client, stamps), and only while a reader is
subscribed. `hypha record` records that stream like any other.

**Replay (decision).** `hypha play` never publishes service traffic by default. Two later
modes use the recorded `calls` stream:

- `--serve <service>`: the player is the server, for a client under test. It answers in
  recorded order and refuses on a mismatch of the request.
- `--call <service>`: the player is the client, for a server under test. It sends the
  recorded requests at their stamps and reports responses that differ.

Neither is in the first version. A system replayed today with services between its
participants still runs deterministically, by the lockstep rule above, because the calls
are re-made live.

## 5. Integrity and failure semantics

**Frames (decision).** Both directions carry the full frame header and its checks, and
the header layout is unchanged (communication-integrity.md §2, FSR-09/10). The CRC's start
value binds the frame to its role:

- A request is sealed for `<service>#request`.
- A response is sealed for `<service>#response/<client id>/<request sequence>`.

So a response verifies only against the request it answers. It cannot pass as a request,
as a topic frame, or as the answer to another call. The spike checks all three
(`a_framed_round_trip...`: a response does not verify against the next sequence or as a
request). The issue proposed adding the client id and sequence to the response header.
Putting them in the start value achieves the same binding without a new header field,
and FSR-03's overhead stays 104 bytes. A hypha server drops its `ActiveRequest` when it
replies, and a hypha client drops the pending call when it takes the first response.
Either drop closes the call's channel, so a second response has nowhere to go. The spike
did not test a misbehaving sender that answers twice before the client reads. Stage 1a
tests it, and the response's sequence check refuses the second as `sequence_gap`.

| Case | Client outcome | Detected by |
|---|---|---|
| No server of the stream when the call is sent | `NoServer` | iceoryx2 reports 0 server connections at send (spike: `a_request_that_no_server_takes_is_seen_at_send`) |
| Outstanding limit | `call()` returns `Busy`; nothing sent | the client's count (iceoryx2 `ExceedsMaxActiveRequests`) |
| Request above the bound | `call()` returns `TooLarge` | encode |
| Server refuses, throws, or its step fails | `Refused` | the server. A throw also counts as the server's `Panic` |
| Server exits or destroys its endpoint | `ServerLost`, at the next turn | iceoryx2 disconnects the pending call. The server's teardown notifies its clients |
| Server crashes (SIGKILL) | `ServerLost` at the deadline | Spike: iceoryx2 alone still reports the call connected and keeps the server's slot. The service's node list reports the server's node dead within 1.4 to 3.7 ms. At the deadline the client probes that list once (one probe: 0.55 to 1.8 ms at load 51) and reports `ServerLost` instead of `DeadlineExpired`. |
| Server restarts | New calls reach the new server. No old request is served again | the restart reclaims its dead node, which disconnects the old calls (spike: `is_connected()` false after the reclaim) |
| No answer within the deadline | `DeadlineExpired`. A late response is dropped and counted (`late_responses`) | the client, in framework time |
| Response corrupt, misrouted, repeated, or of another schema | `InvalidResponse` at once, counted in the frame counters | the frame checks above |
| Client gone before the answer | `reply()` reports `Disconnected` to the server, and the server's counter counts it | iceoryx2 `ActiveRequest::is_connected()` |
| A server queue that is full | Cannot happen | iceoryx2 sizes each client's queue to that client's outstanding limit (`port/server.rs:437`, `buffer_size: max_active_requests_per_client`). A request is never dropped silently. |

**Delivery.** A request is executed at most once. A call gets exactly one outcome. Hypha
does not retry, and it does not recognise a retried request: a retry is a new call with a
new sequence. A server that must be idempotent (MRS `goto` sent again) is idempotent in
its logic, as in ROS 1. Exactly-once execution would need a server-side history of call
ids that survives a restart. That is a database, not middleware.

## 6. Policy and resources

**Decision.** A service has a policy, as a topic does (cpp-api.md §7). It is a
`hypha::ServicePolicy` named in one header that both ends include, mapped to the type by
`hypha::ServicePolicyOf<Srv>`, and stamped on the service. An opener with another policy
is refused with `QosMismatch`.

| Field | Default (`defaultService`) | Meaning |
|---|---|---|
| `clients` | 8 (+3 for tools, as observer slots) | iceoryx2 `max_clients` |
| `outstanding` | 2 per client | iceoryx2 `max_active_requests_per_client`. A `Calls` port asks for 1 by default |
| `requestBytes`, `responseBytes` | the encoded size of a fixed-size half, otherwise 4 KiB | the bound of each half. A larger message is `TooLarge` |
| servers | 1, not configurable | `max_servers = 1` |
| responses per request | 1, not configurable | `max_response_buffer_size = 1` |
| fire-and-forget, safe overflow | off, not configurable | `enable_fire_and_forget_requests(false)`; a request is never overwritten |

*Reason.* A bound is required. Without one each service is growable, which is the issue's
"Payload" fault (166 KB of bookkeeping per growable publisher, no `/dev/shm` check).
4 KiB holds every `std_srvs` call and every MRS and camera call. `nav_msgs/GetMap` or a
point-cloud response declares its own policy. The memory follows from the policy: each
client holds `outstanding x requestBytes` and the server about `clients x outstanding x
responseBytes`, so the default is about 64 KiB per side. The pool sizing that iceoryx2
derives from these numbers has not been measured, so stage 1 measures it (section 12).
A server can hold at most `clients x outstanding` requests deferred, by construction.

**Idle cost.** None. A server with no calls sleeps in its request event, and a client with
none outstanding has no deadline in the `WaitSet`. A client wakes on its service's response
event. Each woken client checks its own outstanding calls (at most `outstanding` atomic
loads), so clients of one service wake each other. A per-client event would remove those
wake-ups, at the cost of one event service per client. That waits until a measurement shows
the wake-ups matter.

**Loans.** The first version copies a request or response into its slot, as typed
publishing does today. A typed loan follows the typed loan of topics (PR #261), once that
lands.

## 7. Transport

**Decision: iceoryx2 request-response.** The service is `hypha/srv/<name>@<md5 16 hex>`
with `[u8]` payloads and the frame header as the user header of both directions. It
comes with two event services, `.../request` (servers listen) and `.../response` (clients
listen). The `hypha/` namespace is already the framework's (`hypha/hk/graph/v1`), so
these names cannot meet a topic.

The alternative is two publish-subscribe streams with a correlation id, which is what
all three ports do today.

| Point | iceoryx2 request-response | Two pub/sub streams |
|---|---|---|
| Routing | A response goes to the asking client only | Every client reads every response and drops the others. Any process can answer with a matching id |
| Readiness | Counts servers | "The request topic has a subscriber": `hypha record`, `topic echo` and Foxglove are subscribers too |
| One server | Enforced by iceoryx2 (spike) | Not expressible |
| Server loss | Graceful: the call disconnects. Crash: the service's node list (spike) | Nothing. The client waits for its own deadline |
| Overflow | Per-client queues that cannot overflow | A request topic can overflow, and the loss is a gap the client never sees |
| Frame header and integrity | Fits: user headers on both directions (spike) | Fits: already built |
| Lockstep | A dedicated rule (section 4) | A feedback loop of two topics between callback nodes holds until `--timeout` (#103) |
| Recording and replay | Not recorded. Stage 4 adds an observer stream | Recorded, and replayed into live servers: the hazard |
| Discovery and tools | New: `list_service_ports` must list request-response services | Free, but as topics, which they are not |
| Topic overloading | The same stream key | Free |
| Loans | Later (#261) | #261 |
| Dead-node reclaim | The same node reclaim covers client and server ports (spike) | Existing |
| Risk | Newer iceoryx2 API with no use in hypha yet (`grep -r request_response crates` finds none). Spike: four tests pass on the first run | Mature and tested in hypha |

The topic pair has a real advantage: recording, echo, overloading and loans come free,
with machinery that hypha has already tested. Its faults are routing, readiness, server
loss and the replay hazard, and none of them can be fixed above the transport. These are
exactly the faults that make a service a control interface instead of a convenience. The
request-response pattern removes them. The spike removed the main doubt about it (section
12), and the work the pattern does not give for free is bounded: discovery rows, a
lockstep rule, and an observer stream.

**What iceoryx2 offers that hypha does not use**: response streams (several responses per
request), several servers per service, fire-and-forget, safe overflow, flatbuffer
payloads. They stay off because the scope is deliberately narrow.

## 8. The API

### 8.1 C++: a server in a `CycleNode` (MRS `control_manager`, `switch_tracker`)

ROS 1 (`mrs_uav_managers/src/control_manager/control_manager.cpp:4223`):

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
  std::tie(res.success, res.message) = switchTracker(req.value);
  return true;
}
```

hypha:

```cpp
class ControlManager final : public hypha::CycleNode {
  public:
    hypha::Requests<mrs_msgs::String> switchTracker{*this, "switch_tracker"};
    // ... the other inputs; setTrigger(odometry) in setup()

    void step(const hypha::CycleTime& time) override {
        for (hypha::Request<mrs_msgs::String>& request : this->switchTracker.requests()) {
            if (this->failsafe_triggered_ || this->eland_triggered_) {
                request.reply({.success = false, .message = "can not switch tracker, eland or failsafe active"});
                continue;
            }
            auto [success, message] = this->switchTracker(request->value);
            request.reply({.success = success, .message = message});
        }
        // ... the control step, which sees the switch it just made
    }
};
```

```toml
[ports.servers.switch_tracker]
service = "~/switch_tracker"     # /uav1/control_manager/switch_tracker
```

The switch happens at a step boundary, before the control step of the same iteration.
It cannot interleave with a step. `is_initialized_` disappears: a step does not run
before `start()`.

### 8.2 C++: a chain of calls with a deferred answer (MRS `uav_manager` takeoff)

ROS 1 runs the sequence inside the callback (`uav_manager.cpp:1480-1560`): `switchControllerSrv()`,
`switchTrackerSrv()`, `ros::Duration(0.3).sleep()`, `takeoffSrv()`, each a blocking
`call()`, then `return true` with the response.

hypha: the server defers, and each call is a step of a state machine.

```cpp
class UavManager final : public hypha::CycleNode {
  public:
    hypha::Requests<std_srvs::Trigger> takeoff{*this, "takeoff"};
    hypha::Calls<mrs_msgs::String>     switchController{*this, "switch_controller"};
    hypha::Calls<mrs_msgs::String>     switchTracker{*this, "switch_tracker"};
    hypha::Calls<mrs_msgs::Vec1>       trackerTakeoff{*this, "tracker_takeoff"};
    hypha::Calls<std_srvs::SetBool>    toggleOutput{*this, "toggle_output"};
    hypha::Calls<std_srvs::SetBool>    arming{*this, "arming"};

    void step(const hypha::CycleTime& time) override {
        for (hypha::Request<std_srvs::Trigger>& request : this->takeoff.requests()) {
            if (auto refusal = this->checkTakeoff()) {          // the upstream checks
                request.reply({.success = false, .message = *refusal});
            } else if (this->pending_) {
                request.reply({.success = false, .message = "takeoff in progress"});
            } else {
                this->pending_ = request.defer();
                this->switchController.call({.value = this->takeoffController_});
                this->state_ = State::SwitchController;
            }
        }
        this->advance(time);
    }

  private:
    enum class State { Idle, SwitchController, SwitchTracker, Settle, TrackerTakeoff };

    void advance(const hypha::CycleTime& time) {
        switch (this->state_) {
            case State::Idle: return;
            case State::SwitchController:
                if (auto r = this->switchController.result()) {
                    if (!succeeded(*r)) return this->abort("could not activate the takeoff controller", *r);
                    this->switchTracker.call({.value = this->takeoffTracker_});
                    this->state_ = State::SwitchTracker;
                }
                return;
            case State::SwitchTracker:
                if (auto r = this->switchTracker.result()) {
                    if (!succeeded(*r)) return this->abort("could not activate the takeoff tracker", *r);
                    this->settleUntil_ = time.now + hypha::Duration::fromMillis(300);   // upstream sleeps 0.3 s
                    this->state_ = State::Settle;
                }
                return;
            case State::Settle:
                if (time.now >= this->settleUntil_) {
                    this->trackerTakeoff.call({});
                    this->state_ = State::TrackerTakeoff;
                }
                return;
            case State::TrackerTakeoff:
                if (auto r = this->trackerTakeoff.result()) {
                    if (!succeeded(*r)) return this->abort("takeoff failed", *r);
                    this->pending_->reply({.success = true, .message = "taking off"});
                    this->pending_.reset();
                    this->state_ = State::Idle;
                }
                return;
        }
    }

    template <class Srv>
    static bool succeeded(const hypha::CallResult<Srv>& r) { return r.answered() && r.response().success; }

    template <class Srv>
    void abort(const std::string& what, const hypha::CallResult<Srv>& r) {
        this->toggleOutput.call({.data = false});
        this->arming.call({.data = false});
        this->pending_->reply({.success = false, .message = what + " (" + hypha::describe(r) + ")"});
        this->pending_.reset();
        this->state_ = State::Idle;
    }

    State                                            state_{State::Idle};
    std::optional<hypha::Deferred<std_srvs::Trigger>> pending_;
    hypha::Time                                      settleUntil_{};
};
```

```toml
[ports.servers.takeoff]
service = "~/takeoff"
[ports.clients.switch_controller]
service = "/uav1/control_manager/switch_controller"
timeout_ms = 1000
[ports.clients.switch_tracker]
service = "/uav1/control_manager/switch_tracker"
timeout_ms = 1000
# ...
```

`hypha::describe(result)` gives `answered`, `refused`, `no server`, `server lost`,
`deadline expired` or `invalid response`. Against the current port
(`examples/mrs/managers/uav_manager.cpp:192-215`), `callStep`, `sent_` and the hand-made
timeout disappear: `result()` is empty until the outcome arrives, and every failure is
already an outcome. A `Calls` port holds one call at a time by default (`outstanding(1)`),
as `mrs::Request` does.

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

hypha: the worker thread owns the device (the port's `std::jthread worker_`). The handler
defers the request and hands it to the worker, which answers between two grabs. No lock
guards the device, and the callback runs on the node's thread.

```cpp
void ArenaCameraNode::setup(hypha::Node& node) {
    this->setExposure_ = node.advertiseService<camera_control_msgs::SetExposure>(
        "set_exposure", [this](hypha::Request<camera_control_msgs::SetExposure>& request) {
            this->commands_.push(ExposureCommand{request->target_exposure, request.defer()});
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

`commands_` is the application's own bounded queue. A camera port that keeps upstream's
shape can answer in the handler under a mutex instead: `request.reply({...})`.

### 8.4 `std_srvs`

The SDK ships `std_srvs` (`Empty`, `Trigger`, `SetBool`) as it ships `std_msgs`. Today it
ships no `.srv` type, and the MRS example declares its own `std_srvs`
(`examples/mrs/msg/std_srvs/srv/`). Designated initializers make them short:

```cpp
request.reply({.success = true, .message = "armed"});   // Trigger, SetBool
arming.call({.data = true});                            // SetBool
reset.call({});                                         // Empty, Trigger
```

### 8.5 A client in a plain node, and in a tool or a test

```cpp
auto trigger = node.serviceClient<std_srvs::SetBool>(
    hypha::Service{"/localization/ekf/trigger_node"}, hypha::timeout(hypha::Duration::fromSeconds(1)));
trigger.call({.data = true}, [](const hypha::CallResult<std_srvs::SetBool>& r) {
    if (!r.answered() || !r.response().success) {
        HYPHA_LOG_WARN("trigger_node: {}", hypha::describe(r));
    }
});

// a gtest or a worker thread: no WaitSet runs on this thread
const hypha::CallResult<std_srvs::Trigger> r = takeoffClient.callAndWait({});
```

### 8.6 Tables

| Table | Keys | Meaning |
|---|---|---|
| `[ports.servers.<port>]` | `service` | The name. An `optionalService` port with `service = ""` is disabled, as `optionalTopic` disables a topic port |
| | `per_step` | Requests a step takes, default 8, at most `clients x outstanding`. The rest wait for a later step |
| `[ports.clients.<port>]` | `service`, `timeout_ms` | The name and the deadline. Both are required unless the code gives them |
| | `outstanding` | Calls in flight, default 1 for a `Calls` port, at most the policy's |

The tables also give a plain node's `advertiseService("set_exposure", ...)` its name. The
migration guide's "a service name does not resolve from the ports tables" gap closes.

### 8.7 C ABI (sketch; sizes, ownership and null rules beside each declaration)

```c
HyphaStatus hypha_service_server_create(HyphaNode*, const char* name, size_t name_len,
                                        const HyphaServiceSpec* spec, HyphaServer** out);
HyphaStatus hypha_service_client_create(HyphaNode*, const char* name, size_t name_len,
                                        const HyphaServiceSpec* spec, HyphaClient** out);
/* spec: type name, srv MD5, request and response definition and MD5, policy, deadline. */
HyphaStatus hypha_client_ready(const HyphaClient*, uint8_t* out);
HyphaStatus hypha_client_call(HyphaClient*, const uint8_t* request, size_t len, uint64_t* out_call);
/* Busy, TooLarge, or Ok. Every Ok call id gets exactly one outcome. */
HyphaStatus hypha_client_call_and_wait(HyphaClient*, const uint8_t* request, size_t len,
                                       HyphaCallOutcome* outcome, uint8_t* response,
                                       size_t capacity, size_t* response_len);
/* InvalidState on a thread inside a WaitSet dispatch. */
HyphaStatus hypha_waitset_attach_client(HyphaWaitSet*, HyphaClient*, HyphaCallDone cb,
                                        void* context, void (*release)(void*));
HyphaStatus hypha_waitset_attach_server(HyphaWaitSet*, HyphaServer*, HyphaServe cb,
                                        void* context, void (*release)(void*));
HyphaStatus hypha_cycle_bind_server(HyphaCycle*, HyphaServer*, uint32_t per_step, uint32_t* out_binding);
HyphaStatus hypha_cycle_bind_client(HyphaCycle*, HyphaClient*, uint32_t* out_binding);
HyphaStatus hypha_cycle_image_request(const HyphaCycleImage*, uint32_t binding, size_t index,
                                      HyphaRequest** out);          /* owned by the image */
HyphaStatus hypha_cycle_image_call_result(const HyphaCycleImage*, uint32_t binding, size_t index,
                                          HyphaCallResult* out);
HyphaStatus hypha_cycle_emission_call(HyphaCycleEmission*, uint32_t binding,
                                      const uint8_t* request, size_t len, uint64_t* out_call);
HyphaStatus hypha_request_defer(HyphaRequest*, HyphaRequest** out_owned);
HyphaStatus hypha_request_reply(HyphaRequest*, const uint8_t* response, size_t len); /* consumes an owned one */
HyphaStatus hypha_request_refuse(HyphaRequest*);                                   /* consumes an owned one */
```

`HyphaCallOutcome`: `Answered`, `Refused`, `NoServer`, `ServerLost`, `DeadlineExpired`,
`InvalidResponse`. These are outcomes, not statuses, and `HyphaStatus` 19 stays retired
(as the issue says). `abi.revision` goes up by one.

### 8.8 Rust SDK

The tools need only a client: `hypha_node::sdk::ServiceClient::call_and_wait(&self,
request: &[u8], timeout) -> Result<CallResult, ServiceError>`, typed by the schema it
reads from the store, as `hypha topic echo` reads a topic's. AGENTS.md allows the SDK to
lag a new capability. A Rust server is not needed.

## 9. Tools and test support

- **`hypha service list`** prints one line per stream: the name, type, MD5, server node,
  client count, and calls answered, refused and expired. `hypha service info <s>` adds the
  definition and the policy. **`hypha service call <s> '{data: true}'`** reads the request
  as YAML or JSON, encodes it from the definition in the schema store, calls with
  `--timeout` (default 5 s), and prints the response as YAML, decoded as
  `hypha topic echo` decodes a message. No tool encodes a message from text today, so the
  encoder is new work in stage 1d. It exits non-zero and prints the outcome for any
  outcome except `Answered`. The tool's node is a CLI node, so its clients use the
  observer slots and never take a declared client's slot.
- **Introspection.** `~/introspection` lists the servers and clients of a node with their
  policies (FSR-41). `~/state` counts per endpoint: received, answered, refused,
  unanswered, deferred now, and, per client, each outcome and `late_responses`.
  `hypha top` shows a services panel from them.
- **`StepHarness`.** `harness.request(port, request)` delivers a request to the next step
  and returns a probe that holds the reply, the refusal or the deferral after the step.
  `harness.calls(port)` lists what the step called, and `harness.answer(port, response)`
  or `harness.outcome(port, hypha::CallOutcome::ServerLost)` gives the next step its
  result. A client under test therefore needs no fake server process. The `uav_manager`
  sequence becomes a step test of each failure branch.
- **The ROS bridge.** For a service, the direction ports need is ROS 1 to hypha: a ROS
  operator or tool (`rosservice call /uav1/uav_manager/takeoff`) calls a hypha server.
  The bridge advertises the service on the ROS master and forwards each call. The reverse
  direction, a hypha client and a ROS server, follows when a port asks for it. ROS 1
  service calls are TCPROS connections per call or persistent. The bridge keeps one hypha
  client per ROS service.

## 10. Migration guide rows (replace the stop-and-ask)

| ROS 1 | hypha |
|---|---|
| `nh.advertiseService(name, &C::cb, this)` in a cycle node | a `hypha::Requests<Srv>` port; in the step, `for (auto& r : port.requests()) r.reply({...})`; `[ports.servers.<port>] service = ...` |
| `nh.advertiseService` in a plain or application node | `node.advertiseService<Srv>("<port>", [](hypha::Request<Srv>& r) { r.reply({...}); })` |
| callback `return false` | `r.refuse()` |
| callback that does work over time, nested calls, sleeps | `r.defer()`, keep the `hypha::Deferred<Srv>`, answer it in the step that finishes. Each nested call is a `Calls` port (§8.2) |
| `nh.serviceClient<Srv>(name)` | a `hypha::Calls<Srv>` port in a cycle, or `node.serviceClient<Srv>("<port>", hypha::timeout(d))` in a plain node; `timeout_ms` in `[ports.clients.<port>]` |
| `client.call(srv)` in a callback | in a cycle: `port.call(request)` in one step, `port.result()` in a later one. In a plain node: `client.call(request, onResult)`. A blocking call on the run thread is refused (`InvalidState`) |
| `client.call(srv)` in a worker thread, a test, a tool | `client.callAndWait(request)` |
| `ros::service::waitForExistence(name, t)`, `client.waitForExistence()` | none. Start regardless (FSR-25). A call before the server exists ends `NoServer`. Retry in the logic, or gate on `client.ready()` in the step |
| `client.exists()` | `client.ready()` |
| `ros::service::call(name, srv)` | a client port, as above. Only tools and tests call without one |
| `serviceClient(name, true)` (persistent) | the default. Every client stays connected |
| a service called from inside a callback | a state over steps (§8.2), not a nested wait |
| `std_srvs/*` | shipped by the SDK |
| `rosservice call` | `hypha service call` |

The two rows that point at `cpp-api.md` §8 (request and reply topics with an identifier)
go, and so does the stop-and-ask item "A service whose caller blocks on the reply".

## 11. Staging

Sizes are rough estimates of changed lines including tests, from the sizes of comparable
changes (the cycle `Arrivals` path and the topic overloading PR). They are not measured.

| Stage | Content | Tests | Size |
|---|---|---|---|
| **1a Transport** | `hypha_iceoryx2`: server and client ports, the two events, sealing with the role codes, the policy stamp, the stream key, schema records for both halves, discovery rows | each row of the failure table at transport level, in two processes (the spike's tests become these); pool sizes measured against the policy | 1,500 |
| **1b Runtime, plain nodes, C ABI, C++** | `hypha_node`: outstanding calls, deadlines in the `WaitSet`, outcomes, the liveness probe at the deadline, the dispatch-thread refusal; C ABI; `advertiseService`, `serviceClient`, `Request`, `Deferred`, `CallResult`, `callAndWait`; `std_srvs` in the SDK; tables; docs (`interface-specification.md`, `cpp-api.md` §8, `features.md`, `execution-model.md`, `communication-integrity.md`) | the failure table through C++; a deferred reply from another thread; a destroyed `Deferred` refuses; installed-SDK flow | 2,500 |
| **1c Cycle ports and lockstep** | `Requests<Srv>`, `Calls<Srv>`; replies and calls sent after the step; refusal on a failed step; the admission rule under simulated time; the client's upstream edge; `StepHarness` support | a client and a server cycle in lockstep replay identically across two runs and never reach `--timeout`; a failed step refuses its requests; harness tests | 2,000 |
| **1d Tools and the first users** | `hypha service list/info/call` (Rust SDK client); the MRS host and the rclcpp shim move onto it; `mrs_bridge/Call` and `ServiceCall` are deleted; the EKF trigger starts on bus traffic | the five MRS scenarios; the Autoware EKF trigger; a CLI test of `service call` | 1,500, of which about 600 deleted |
| 2 Introspection | `~/state` and `~/introspection` rows, `hypha top` panel | | 600 |
| 3 Requests as a trigger | a `Requests` port can trigger a cycle, with no silence limit and no fallback | the trigger rules of execution-model.md §6.6 for a request | 500 |
| 4 Recording and replay | the observer `calls` stream; `hypha play --serve`, `--call` | a recorded call served back to a client under test; a recorded call replayed into a server | 1,500 |
| 5 ROS bridge | ROS 1 to hypha for services | a `rosservice call` reaches a hypha server in the bridge's container test | 1,000 |
| 6 Large payloads | typed loans for requests and responses (after #261); a per-client wake event if measured to matter | | 500 |

**The smallest version that unblocks the flight and the camera** is 1a to 1c. The camera
needs only 1a and 1b. Stage 1d is the first use, and it deletes the two hand-made wire
formats (`features.md` rule 4: no second path). Each stage is a pull request of its own.

**Requirements.** New entries, for the owner:

- **FSR-42** The software shall provide request and response between nodes, typed by a
  ROS 1 `.srv` definition. Each service stream shall have at most one server, and a second
  server shall be refused at creation. Each call shall end in exactly one outcome, which
  distinguishes answered, refused, no server, server lost, deadline expired and invalid
  response. No request shall be lost without an outcome.
- **FSR-43** A call shall carry a deadline in framework time. A call that waits for its
  outcome shall be refused on a thread that runs a dispatch.
- **FSR-44** A request made by a replay participant shall be served at a deterministic
  step of its server, and a replay with services between participants shall repeat
  exactly.

Existing ones that widen: FSR-09/10 (service frames, the role codes), FSR-13 (a request
or a response wakes its reader), FSR-22/41 (services in the tools and the snapshot),
FSR-24 (service traffic is not recorded until stage 4; say so), FSR-26 (server ports in
the reclaim), NFR-05 (the service bounds). FSR-21 already covers `.srv`.

## 12. The spike

`crates/hypha_iceoryx2/tests/service_spike.rs` with a server process
`tests/support/service_server_helper.rs`, iceoryx2 0.10.1, macOS (aarch64), run with
`cargo nextest run -p hypha_iceoryx2 --test service_spike --no-capture` (a private
`HYPHA_DOMAIN` per test process). Four tests pass, in three runs.

- The frame header is the user header of both directions (`std_srvs/SetBool`: a 1-byte
  request, a 7-byte response). Every response verifies against its own request, and does
  not verify against the next sequence or as a request.
- The server process creates the one server and then sleeps in the request event (10 s
  timeout as a lost-wake guard). It answered 2,000 sequential calls at the times below,
  so it was woken by the event and not by its timeout.
- A second server on the name is refused: `ExceedsMaxSupportedServers`.
- A call with no server: 0 server connections at send, `is_connected()` false.
- The server process is killed (SIGKILL) while it holds a request. iceoryx2 alone keeps
  reporting the call connected (`is_connected()` true, no response), the dead server keeps
  its slot (`number_of_servers() == 1`), and a new server under another node name is
  refused. The service's node list reports the server's node dead 1.4, 3.7 and 2.6 ms
  after the kill (the first probe each time). One probe of that list took 0.55 to 1.8 ms.
  After the dead node is reclaimed, the call reports disconnected, the slot is free, and a
  new server is created.
- `ActiveRequest` and `PendingResponse` are `Send`, so a deferred reply can be answered
  from a worker thread.

Round trip of a `SetBool` call between two processes, 2,000 calls, measured in the client
(send, notify, wait on the response event, receive, verify):

| Run | Load average (1, 5, 15 min) | Median | p90 | p99 | Max |
|---|---|---|---|---|---|
| 1 | 50.9 38.9 26.3 | 51 µs | 236 µs | 1.26 ms | 12.9 ms |
| 2 | 52.9 39.7 26.8 | 107 µs | 285 µs | 4.82 ms | 36.3 ms |
| 3 | 52.9 39.7 26.8 | 160 µs | 247 µs | 1.46 ms | 14.2 ms |

The host was heavily loaded, so these are an upper picture, not a benchmark. The spike did
not measure the topic pair on the same host for comparison.

## 13. Not in scope, and why

- **Parameters as services** (`get_param`, `set_param`, `dynamic_reconfigure`).
  Parameters are an immutable description read once (FSR-18). A runtime change through a
  service would be a second configuration path. A node that needs a runtime setting
  serves a setter of its own, as the camera does.
- **Actions.** An action is a goal, feedback, a result and a cancel. In hypha it would be
  a goal service with a deferred reply (the result), a feedback topic, and a cancel
  service, all on these primitives. It needs a goal id across them and a rule for a
  preempted goal. That is a library above the core, worth doing once a port needs one
  (the camera's two grab actions are dropped today).
- **Response streams, several servers, fire-and-forget.** iceoryx2 offers them. None of
  the ports needs them, and each would weaken "exactly one outcome from the one server".
- **A blocking call inside a run, and `waitForExistence`.** Section 3.
- **Service wildcards.** A call is addressed to one server, and a pattern has no meaning
  there (already the rule in interface-specification.md: a wildcard never follows a
  request-response service).
- **Exactly-once execution.** Section 5.

## 14. Open risks

- **The maturity of iceoryx2's request-response in 0.10.1.** The spike covers creation,
  the one-server limit, a framed round trip, no server, and a crash. It does not cover
  creation races of servers and clients in many processes, `hypha clean` of
  request-response services, the open-file count of three services per call name, or
  Linux. Stage 1a tests these first. A vendored iceoryx2 (open PR, for another reason)
  would let a fix land without waiting for a release.
- **Pool memory.** iceoryx2 derives the request and response pools from
  `max_clients`, `max_active_requests_per_client` and the slice length. The design's
  numbers in section 6 are an estimate until stage 1a measures them against `/dev/shm`.
- **The liveness probe's cost.** 0.55 to 1.8 ms per probe at load 51, on the run thread.
  It runs only when a deadline expires with a call still connected. A system with many
  expiring calls (a missing server under load) pays it often. A bounded rate, or a move to
  the housekeeping refresh of PR #266, is the fallback.
- **A crashed server blocks its service until its node restarts.** This follows from
  FSR-26. Under `hypha launch` with restarts it is a restart's time. Without a supervisor
  the service stays down and the refusal says why. Stealing a dead node's slot was
  rejected because it would break FSR-26 for one port kind.
- **Lockstep admission.** The rule in section 4 rests on "data before progress" and on the
  player's acknowledgement order. It has been reasoned through but not run. Stage 1c's
  two-run identity test is the check. A client that is a triggered cycle triggered by a
  response, and chains of deferred replies across three nodes, need their own tests.
- **Shared response event.** The clients of one service wake each other. This is harmless
  at 8 clients. A service with many clients should be measured before relying on it.
- **Naming.** `Requests`, `Calls`, `Request`, `Deferred`, `advertiseService` and
  `serviceClient` are proposals. The ROS 1 names are kept where a porter types them.
  `hypha/srv/<name>@<md5>` has not been checked against iceoryx2's service-name length
  limit for long names.
