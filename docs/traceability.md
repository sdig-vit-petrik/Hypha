# Requirements traceability

Each requirement of [requirements-specification.md](requirements-specification.md)
and the evidence that verifies it. Test names are files under `<crate>/tests/` (Rust)
or `crates/hypha-cxx/test/` (C++). **Gap** marks a requirement whose stated method has
no automated evidence yet; the gap column says what is missing. The letters and
numbers in brackets (R1, P4, ...) are the items of the former backlog document.
Updated 2026-11-15.

## Functional requirements

| ID | Verified by | Gap |
|---|---|---|
| FSR-01 | `hypha_iceoryx2/conformance`, `hypha/framework_stamp_cross_language` (C++ and Rust processes), `hypha_iceoryx2/domain_isolation` | |
| FSR-02 | Analysis: iceoryx2 writes a sample once and appends an offset per connection | No test measures publish time against subscriber count (R7). |
| FSR-03 | `hypha_ffi/cycle_single_copy`, `hypha_ffi/pubsub_framed_samples`, `hypha_wire/frame` | |
| FSR-04 | `hypha_msgdef/golden_md5` (every repository message), `test_ros1_codec_encode`, `hypha_msgs/messages`, `hypha_wire/schema_record` | |
| FSR-05 | `hypha_core/frame_validation`, `test_typed_schema_identity`, `hypha/client_facade_round_trip`; R4 harness (wrong schema hash: all rejected and counted) | |
| FSR-06 | `hypha_iceoryx2/payload_capacity`, `hypha_ffi/publisher_payload_capacity`, `test_publish_receive_status` | |
| FSR-07 | `hypha_iceoryx2/latch_history`, `latch_publisher_lifetime`, `latch_raw_payload`, `hypha_ffi/latch_late_joiner_integrity` | |
| FSR-09 | `hypha_wire/mandatory_integrity_footer`, `hypha_wire/frame`, `hypha_wire/publisher_identity` | |
| FSR-10 | `hypha_core/frame_validation`, `hypha_wire/sequence_validator`, `hypha_ffi/subscriber_integrity_stats`, `hypha_ffi/delivery_transport_age`; R4 harness (corrupt, future, reordered frames from a peer process) | The zero-copy window after the check is accepted (architecture §3.4). |
| FSR-11 | `hypha_cycle/input_judgement`, `hypha_cycle/input_supervision`, `test_input_supervision` | |
| FSR-12 | `hypha_ffi/subscriber_delivery_mode`, `test_subscriber` | |
| FSR-13 | `hypha_iceoryx2/notification`, `hypha_ffi/waitset_wakeups`, `hypha_iceoryx2/waitset_wakeup_recovery` | |
| FSR-14 | `hypha_iceoryx2/housekeeping_creates_no_thread`, `hypha_ffi/waitset_cross_thread_stop` | |
| FSR-15 | `hypha_core/timer_missed_count`, `hypha_core/timer_reset`, `hypha_time/cadence_policies` | |
| FSR-16 | `hypha_time/clock`, `hypha_time/cross_process_monotonic`, `test_clock` | |
| FSR-17 | `hypha_ffi/sim_time_environment`, `hypha_ffi/sim_time_waitset_turn`, `test_clock_driver`, `test_time_jump`; MRS flights in `examples/check.sh` | |
| FSR-18 | `hypha_core/parameters`, `hypha_core/parameter_constraints`, `hypha_ffi/parameter_boundary`, `test_parameters` | |
| FSR-19 | Inspection: only `hypha_ffi` depends on `hypha_iceoryx2` (architecture); `check.sh` checks the generated header | No automated check of the dependency rule. |
| FSR-20 | The C++ suite, the `hypha` crate tests, `hypha/framework_stamp_cross_language` | |
| FSR-21 | `hypha_msggen/*`, `hypha_msgs/messages`, the installed SDK flow (`scripts/test-installed-sdk.sh`) | |
| FSR-22 | `hypha_tools/echo_plot_cli`, `hypha_tools/topic_follower`, `hypha_ffi/introspection_rows` | The O(1) clause has no measurement (R7). |
| FSR-23 | `hypha_ffi/schema`, `hypha_ffi/schema_hash_resolution`, `hypha_iceoryx2/schema_retention` | |
| FSR-24 | `hypha_tools/test_recorder_player_core`, `record_sync`, `utc_replay_bus`, `play_flow_control` | |
| FSR-25 | `hypha_iceoryx2/start_order_matrix` | |
| FSR-26 | `hypha_iceoryx2/schema_retention` (a closed topic leaves no record), `test_handle_lifetime`; an MRS flight leaves nothing in `/tmp/iceoryx2` | No test checks that a clean shutdown leaves no segment. |
| FSR-27 | `hypha_iceoryx2/stale_resource_reclaim`, `hypha_node/peer_kill_isolation` (SIGKILL of a publisher, of a reader and of a reader that holds samples; a restarted publisher: no loss, no rejection, no refused publish for the survivors); R3 harness, by hand | No test kills a publisher that holds an unsent loan, or a process between two sends of one cycle step. |
| FSR-28 | `hypha_ros_bridge/direction_routing`, `packet_framing`, `automatic_typed_ros_ingress` | |
| FSR-29 | `hypha_ros_bridge/manifest_validation`, `automatic_routing` | |
| FSR-30 | `hypha_ros_bridge/automatic_echo_suppression`, `ros_publisher_identity` | |
| FSR-31 | `hypha_tools/test_foxglove_protocol` | |
| FSR-32 | `hypha_tools/test_foxglove_protocol` | |
| FSR-35 | `hypha_tools/launch_supervisor`, `launch_session_lifecycle`, `test_launch_units` | |
| FSR-36 | `hypha_log/*`, `test_log_*` | |
| FSR-37 | `hypha_ffi/sim_time_rewind`, `hypha_tools/play_restart`, `hypha_core/timer_time_jump` | |
| FSR-38 | `hypha_cycle/*`, `hypha_ffi/cycle_*`, `test_cycle_node`, `test_cycle_step_faults` | |
| FSR-39 | `hypha_cycle/input_supervision`, `test_input_supervision` | |
| FSR-40 | `hypha_iceoryx2/domain_isolation`, `hypha/domain` | |
| FSR-41 | `hypha_ffi/introspection_snapshot`, `test_node_introspection` | |

## Non-functional requirements

| ID | Verified by | Gap |
|---|---|---|
| NFR-01 | Inspection: `libhypha_ffi` is the one deliverable; the C++ SDK uses it through `include/hypha/backend/` | |
| NFR-02 | Rust core; `unsafe` forbidden by lint level outside `hypha_ffi` (the C boundary), `hypha_sys` (OS calls), the test-only `hypha_testkit/src/env.rs` and the C boundary module `hypha_testing_mcap/src/ffi.rs` of the test-only MCAP reader; `hypha_node` and the tools forbid it; sanitizer and Miri runs, 2026-11-11 (R1); `scripts/miri.sh` (Miri over the crates without transport and the node-free `hypha_node` tests, CI job `test:miri`); `scripts/sanitize.sh` (C++ tests) | `test:miri` has not run on the runners yet; the sanitizers are not in CI; Miri cannot run a node, `hypha_ffi` or `hypha_sys` (P4). |
| NFR-03 | None automated | macOS 3-hop p99 2–6 ms; no Linux measurement (R7). |
| NFR-04 | None automated | R7. |
| NFR-05 | `hypha_iceoryx2/subscriber_depth`, `resource_qos`, `shm_capacity`, `hypha_cycle/cycle_history` | Growth under sustained overload is not measured (R8). |
| NFR-06 | None | R8. |
| NFR-07 | Inspection: iceoryx2 is brokerless; no hypha daemon exists | |
| NFR-08 | Workspace lints (panic, indexing, arithmetic); property tests of every input parser and decoder (`*_properties` in `hypha_wire`, `hypha_msgdef`, `hypha_core`, `hypha_msgs`, `hypha_tools`); fuzzing, 2026-11-11 (R2); `scripts/fuzz.sh` (a `fuzz/` target per parser and decoder, CI job `test:fuzz`, scheduled or by hand) | `test:fuzz` has not run on the runners yet, and only as a 30 s smoke run per target (P4). |
| NFR-09 | `hypha_ffi/node_health` (a foreign callback status 7), `cycle_step_exception_is_logged`; abort: release library links no unwinder entry (`_Unwind_RaiseException`), checked by inspection | |
| NFR-10 | `scripts/coverage.sh` (`--fail-under-lines 80` per crate, CI) | The apps have no coverage gate. |
| NFR-11 | `hypha_ffi/startup_latency` (node creation to first publish); exec to first sample measured 2026-11-11 (R9) | |
| NFR-12 | `hypha_iceoryx2/payload_capacity`, `hypha_ffi/publisher_payload_capacity`, `examples/large_payload` | |
| NFR-13 | `hypha_wire/sequence_validator`, `hypha_core/frame_validation`, `identifier_allocation`, `timer_missed_count`, `hypha_cycle/cycle_history` | |
