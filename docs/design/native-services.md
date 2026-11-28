# Native services

Design for #132: typed request/response between nodes from ROS 1 `.srv` definitions.
Status: accepted design; **Stage 1a** (transport in `crates/hypha_iceoryx2/src/service/`) is implemented, Stages 1b+ are pending.

## 1. Identity and discovery

- **Stream key:** `<service>@<16-hex schema hash>` (from `.srv` MD5 via `hypha_wire::schema_hash_from_md5`), matching topic overloading (#266). Names resolve like topics (absolute, relative, `~/`, max **219 bytes**, no `@`, `/hypha/` reserved except `/hypha/intercept/`).
- **One server per name (across all versions):** enforced atomically by a single-notifier event `hypha/srv/<name>` carrying attribute `hypha.service_lock`. A second server fails with `ResourceLimit` naming the holder's type, MD5, node, and pid.
- **Dead-node reclaim:** a new server or client reclaims a dead holder (via iceoryx2's monitor lock, within 500 ms for a lingering server port). Live holders are never preempted.
- **Late server:** starting a client before its server is not an error (FSR-25). Calls wait in the client's outstanding slots until a server appears (checked on graph event and each turn) or time out as `NoServer`. Mismatched server versions are reported via the same "other schemas" mechanism as topic overloading.

## 2. Execution model

### Server
One fixed Rust slot table (`clients × outstanding` slots) with two front ends:
- **Cycle (`hypha::Requests<Srv>`):** requests captured per step (`per_step`, default 8); answers sent after the step succeeds, in order with outputs. Unread requests stay queued; read-but-unanswered requests are refused (`service.unanswered`). A failed step or throwing handler ends read requests as `ServerLost("server step failed")`.
- **Plain / `ApplicationNode` (`node.advertiseService<Srv>(name, handler)`):** callback runs on the `WaitSet` thread between turns.

Each `hypha::Request<Srv>&` is resolved by `reply(resp)`, `refuse([reason])`, or `defer() -> hypha::Deferred<Srv>`.
- `Deferred` holds a slot index + weak server ref (never owns `ActiveRequest`, so destroying the server always releases the server port and notifies clients with `ServerLost`). Move-only, `explicit operator bool`, thread-safe `reply`/`refuse` (any-thread reply is non-deterministic in replay), refuses on drop if unanswered. Abandoned client slots are dropped each turn (`ActiveRequest::is_connected()`).

### Client
- **Cycle (`hypha::Calls<Srv>`):** `call(req)` stages a request (sent on step success); outcomes arrive in a later step via `result()` / `results()`. Unread outcomes are dropped at step end (`results_unread`).
- **Plain node (`node.serviceClient<Srv>(name)`):** `call(req, onResult)` sends immediately; `onResult` runs on a later `WaitSet` turn.
- **Blocking (`node.serviceClient<Srv>(name, hypha::blocking)`):** `callAndWait(req)` and `waitForServer(timeout)` for tools, tests, and worker threads. Refused with `InvalidState` on any `WaitSet` dispatch thread. No library threads (FSR-14).
- **Sequencing (`hypha::Sequence`, Stage 1c):** ~200-line header-only C++23 coroutine (`co_await calls.call(req)`, `co_await hypha::sleepFor(d)`, `co_await hypha::until(pred)`), advanced strictly by `seq.advance()` in `step()`.

`call()` returns `[[nodiscard]] CallStatus`: `Sent`, `Busy` (at outstanding limit, including timed-out calls still held by the server), or `TooLarge`. Every `Sent` call produces exactly one `CallOutcome`:

| Outcome | Cause | `mayHaveRun()` | `safeToRetry()` |
|---|---|---:|---:|
| `Answered` | Server replied (`ok() == true`) | yes | no |
| `Refused(reason)` | `refuse()`, unanswered read/dropped `Deferred`, response > bound, invalid request | no | yes |
| `NoServer` | No live server of this stream before timeout | no | yes |
| `NotSent` | Client step that staged the call failed | no | yes |
| `ServerLost(reason)` | Server exited/crashed, handler threw/step failed, invalid response | yes | no |
| `Timeout` | No answer within framework-time timeout (late replies dropped as `late_responses`) | yes | no |

## 3. Time, lockstep, recording, replay

- **Stamps & lockstep:** requests carry client `framework_sent_nanos`; responses carry server step/callback time. A server step at deadline `D` admits requests strictly `< D`, ordered by `(stamp, client_node, client_port, sequence)`. Participant clients and servers are mutual lockstep upstreams with strict cuts (no deadlock, even within one `WaitSet`). `execution-model.md` §6.6 widens so an upstream that feeds a step only requests or responses reports `T - 1 ns`; one that feeds it a topic still reports `T`, since a triggered step reads its topic inputs through `T` (#231). Timeouts trigger at the first client step with `D >= S + timeout`.
- **Recording:** `hypha record` does **not** record request-response services (prevents replaying calls into live servers). Instead, clients publish `hypha_msgs/ServiceCall` to a call-log topic when subscribed (one atomic check when idle); `hypha record` records it, `hypha play` never republishes it, and `hypha top` reads it. `--serve` / `--call` replay modes wait for Stage 4.

## 4. Wire integrity and failure semantics

- **Sealing:** unchanged frame header (FSR-09/10, 104 B overhead), CRC seeded with role separator U+001F (`␟`):
  - Request: `<service>␟request`
  - Response: `<service>␟response␟<client_id>␟<request_sequence>`
- **Response flags (inside CRC):** normal reply, `FLAG_REFUSED` (UTF-8 reason payload), or `FLAG_FAILED` (maps to `ServerLost(reason)`). Responses echo the request sequence; neither direction counts `sequence_gap`.
- **Crash detection:** graceful server exit disconnects pending calls immediately. On SIGKILL, iceoryx2 keeps calls connected; at timeout of a still-connected call, the client runs a rate-limited liveness probe (`PortFactory::nodes`, ≤1/s per service, cached only while server node is unchanged) and reports `ServerLost` instead of `Timeout`. `Iox2Client::abandon` only drops a call once the server is confirmed dead or absent.
- **Delivery:** at-most-once execution, exactly-once outcome, no automatic retries.

## 5. Policy, transport, and resources

`hypha::ServicePolicy` (mapped by `ServicePolicyOf<Srv>`, stamped on the service; mismatch → `QosMismatch`):
- Defaults (`defaultService`): `clients = 8` (+ 3 CLI observer slots for `_hyphacli_*`, counted separately), `outstanding = 2` (cycle `Calls` default 1), `requestBytes`/`responseBytes` = exact encoded size for fixed-size halves else 4 KiB, `timeout = 1 s` (overridable via `timeout_ms` or `hypha::timeout(d)`). Large payloads (>4 KiB) belong on latched topics unless a custom policy is declared.
- **iceoryx2 services per stream:**
  - `hypha/srv/<name>@<hash>` (request-response, `[u8]` payload, frame header as user header)
  - `hypha/srv/<name>@<hash>/request` & `/response` (wake-up events)
  - `hypha/srv/<name>` (1-notifier server lock event, attribute `hypha.service_lock`)
- **Fixed iceoryx2 settings:** `DiscardData` backpressure on both ends (never `RetryUntilDelivered`), `max_servers = 1`, `max_nodes = clients + observers + 2` (extra slot for `hypha service list`), `max_response_buffer_size = 1`, `max_borrowed_responses_per_pending_response = 1`, `max_loaned_responses_per_request = 1`, `max_loaned_requests = 1`, fire-and-forget and safe-overflow off. Insufficient FDs (~14–20 per endpoint) return `OutOfDescriptors`.

## 6. Configuration, C ABI, and tools

**TOML tables** (missing required table → exit 78):
- `[ports.servers.<port>]`: `service` (`""` disables with `optionalService`), `per_step` (default 8).
- `[ports.clients.<port>]`: `service`, `timeout_ms`, `outstanding` (default 1).
- SDK ships `std_srvs` (`Empty`, `Trigger`, `SetBool`); a deleted `call(Srv&)` overload guides ROS 1 porters.

**C ABI sketch** (bumps `abi.revision` by 1; `HyphaStatus` 19 stays retired; `HyphaCallStatus` / `HyphaCallOutcome` are separate enums):

```c
HyphaStatus hypha_service_server_create(HyphaNode*, const char* name, size_t len, const HyphaServiceSpec*, HyphaServer** out);
HyphaStatus hypha_service_client_create(HyphaNode*, const char* name, size_t len, const HyphaServiceSpec*, uint8_t blocking, HyphaClient** out);
HyphaStatus hypha_client_ready(const HyphaClient*, uint8_t* out);
HyphaStatus hypha_client_pending(const HyphaClient*, uint8_t* out);
HyphaStatus hypha_client_call(HyphaClient*, const uint8_t* req, size_t len, HyphaCallStatus*, uint64_t* out_call);
HyphaStatus hypha_client_call_and_wait(HyphaClient*, const uint8_t* req, size_t len, HyphaCallResult*, uint8_t* resp, size_t cap, size_t* resp_len);
HyphaStatus hypha_client_wait_for_server(HyphaClient*, int64_t timeout_nanos, uint8_t* out);
HyphaStatus hypha_waitset_attach_client(HyphaWaitSet*, HyphaClient*, HyphaCallDone, void* ctx, void (*release)(void*));
HyphaStatus hypha_waitset_attach_server(HyphaWaitSet*, HyphaServer*, HyphaServe, void* ctx, void (*release)(void*));
HyphaStatus hypha_cycle_bind_server(HyphaCycle*, HyphaServer*, uint32_t per_step, uint32_t* out_binding);
HyphaStatus hypha_cycle_bind_client(HyphaCycle*, HyphaClient*, uint32_t* out_binding);
HyphaStatus hypha_cycle_image_request(const HyphaCycleImage*, uint32_t binding, size_t idx, HyphaRequestRef* out);
HyphaStatus hypha_cycle_image_call_result(const HyphaCycleImage*, uint32_t binding, size_t idx, HyphaCallResult* out);
HyphaStatus hypha_cycle_emission_call(HyphaCycleEmission*, uint32_t binding, const uint8_t* req, size_t len, HyphaCallStatus*, uint64_t* out_call);
HyphaStatus hypha_request_reply(HyphaRequestRef, const uint8_t* resp, size_t len);
HyphaStatus hypha_request_refuse(HyphaRequestRef, const char* reason, size_t len);
HyphaStatus hypha_request_defer(HyphaRequestRef, HyphaDeferred** out);
HyphaStatus hypha_deferred_reply(HyphaDeferred*, const uint8_t* resp, size_t len);
HyphaStatus hypha_deferred_refuse(HyphaDeferred*, const char* reason, size_t len);
void        hypha_deferred_destroy(HyphaDeferred*);
```

- **CLI (`hypha service`, Stage 1b):** `list`, `info <s>`, `call <s> [yaml/json]` (prints template if no body; warns if server is on paused sim time; uses CLI observer slot; server logs CLI calls at INFO with pid).
- **Rust SDK:** blocking `hypha_node::sdk::ServiceClient::call_and_wait` only (for tools).
- **`StepHarness` (Stage 1c):** `request(port, req)`, `calls(port)`, `answer(port, resp)`, `outcome(port, ...)`.

## 7. Staging, requirements, and evidence

| Stage | Scope | Status |
|---|---|---|
| **1a Transport** | `hypha_iceoryx2::service`: ports, name lock, dead reclaim, role CRC sealing, slot table, liveness probe, listing | Done (`#270`) |
| **1b Runtime, C ABI, C++, CLI** | `hypha_node` runtime, blocking clients, C ABI, `advertiseService`/`serviceClient`, `std_srvs`, call log, `hypha service` | Next |
| **1c Cycles & lockstep** | `Requests<Srv>`, `Calls<Srv>`, strict-cut lockstep + §6.6 amendment, `StepHarness`, `hypha::Sequence` | Pending |
| **1d Migration** | Port MRS host & Autoware shim; delete `mrs_bridge/Call` and `ServiceCall` | Pending |
| **2–6 Follow-ups** | Introspection/`hypha top` (2), `Requests` trigger (3), `hypha play --serve/--call` (4), ROS 1 bridge (5), typed loans (6) | Pending |

**Requirements:** **FSR-42** (typed `.srv` request/response, single server per name, 6 distinct outcomes, no silent loss), **FSR-43** (framework-time timeout, blocking refused on dispatch thread), **FSR-44** (deterministic strict-cut replay); widens FSR-09/10/13/22/24/26/41 and NFR-05.

**Stage 1a measurements** (`crates/hypha_iceoryx2/tests/service_transport.rs`):
- 2,000 `SetBool` round trips (2 processes): median 53–92 µs (macOS), 268 µs (Linux VM container).
- SIGKILL server detected by probe: ~2.7–2.8 ms; single probe ~1.0–1.3 ms (cached hit 2 µs).
- Memory (Linux): server 549 KiB (formula: 528 KiB), client 24 KiB; FDs: 13–14 (Linux), 18–20 (macOS).

## 8. Out of scope and open risks

- **Out of scope:** parameters-as-services (FSR-18), actions (library above core), response streams, multi-server, fire-and-forget, blocking inside a run, service wildcards, exactly-once execution.
- **Open risks:** iceoryx2 0.10.1 request-response maturity on RT target; killed client holding server slots until dead-node reclaim (not yet tested in 1a); probe latency (0.55–1.8 ms on run thread, ≤1/s); shared response event cross-client wake-ups at high client counts; lockstep §6.6 interaction (tested first in 1c).
