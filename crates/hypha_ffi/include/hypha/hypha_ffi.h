/* The C ABI for hypha. A callback sample lives only for its callback. */

#ifndef HYPHA_FFI_H
#define HYPHA_FFI_H

#define HYPHA_ABI_REVISION 1

#include <stddef.h>
#include <stdint.h>

// The SDK default of `cache_nanos`: dynamic transform history per frame.
#define HYPHA_DEFAULT_TF_CACHE_NANOS 10000000000

// The transform was found and written.
#define HYPHA_TF_FOUND 0

// A frame name is empty or only `/`.
#define HYPHA_TF_INVALID_FRAME 1

// No transform names one of the frames.
#define HYPHA_TF_UNKNOWN_FRAME 2

// Both frames are known, but no chain of transforms connects them.
#define HYPHA_TF_NOT_CONNECTED 3

// The time is older than a transform on the chain still holds.
#define HYPHA_TF_BEFORE_BUFFER 4

// The time is newer than the newest sample of a transform on the chain.
#define HYPHA_TF_AFTER_BUFFER 5

// Freshness verdict: no sample admitted since the binding was created.
#define HYPHA_CYCLE_VERDICT_NEVER_ARRIVED 0

// The newest admitted sample is within the silence limit (for a latest binding with
// none retained, its held sample). A queued binding with no new sample is usable.
#define HYPHA_CYCLE_VERDICT_USABLE 1

// The newest admitted sample is older than the silence limit or stamped after the
// capture clock.
#define HYPHA_CYCLE_VERDICT_STALE 2

// Fault bit: the input is stale, its newest sample older than the silence limit.
#define HYPHA_FAULT_STALE 1

// Fault bit: the alive counter unchanged within the stagnation limit.
#define HYPHA_FAULT_STAGNANT 2

// The SDK default of `HyphaCycleBindingOptions::max_samples_per_iteration`.
#define HYPHA_DEFAULT_ARRIVALS_PER_STEP 8

// A configuration value that the parameter file holds.
#define HYPHA_CONFIG_SOURCE_PARAMETER 0

// A configuration value that application code set.
#define HYPHA_CONFIG_SOURCE_APPLICATION 1

// A configuration value that the command line gives.
#define HYPHA_CONFIG_SOURCE_COMMAND_LINE 2

// A configuration value taken from its default, because the file did not set it.
#define HYPHA_CONFIG_SOURCE_DEFAULT 3

// The port is a subscribing endpoint: its topic is in the `[ports.inputs.<name>]` table.
#define HYPHA_PORT_INPUT 0

// The port is a publishing endpoint: its topic is in the `[ports.outputs.<name>]` table.
#define HYPHA_PORT_OUTPUT 1

// The topic must be set and non-empty: the endpoint cannot be switched off.
#define HYPHA_PORT_TOPIC_REQUIRED 0

// An unset topic or `topic = ""` gives a disabled endpoint.
#define HYPHA_PORT_TOPIC_OPTIONAL 1

// `HyphaConfigValue::kind` of a boolean.
#define HYPHA_CONFIG_VALUE_BOOL 0

// `HyphaConfigValue::kind` of a whole number.
#define HYPHA_CONFIG_VALUE_INT 1

// `HyphaConfigValue::kind` of a number with a fraction.
#define HYPHA_CONFIG_VALUE_FLOAT 2

// `HyphaConfigValue::kind` of text.
#define HYPHA_CONFIG_VALUE_TEXT 3

// The SDK default of `HyphaSubscriberOptions::no_publisher_warning_after_nanos`; the
// ABI value zero disables the warning.
#define HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS 5000000000

// What the callback asks the run loop to do next.
#define HYPHA_ACTION_CONTINUE 0

// Stop the run and report a requested stop.
#define HYPHA_ACTION_STOP 1

// Stop the run and report a failure.
#define HYPHA_ACTION_FAIL 2

// Why a run returned.
#define HYPHA_RUN_REASON_STOPPED 0

// Building the native attachments failed. No callback ran.
#define HYPHA_RUN_REASON_SETUP_FAILED 1

// The run failed after it started.
#define HYPHA_RUN_REASON_RUNTIME_FAILED 2

// A bounded run reached its time limit. The application asked for no stop.
#define HYPHA_RUN_REASON_DEADLINE 3

// A subscription that runs when its subscriber receives a sample.
#define HYPHA_ATTACH_SUBSCRIPTION 0

// A periodic registration on framework time, owned by the `WaitSet`.
#define HYPHA_ATTACH_TIMER 2

// What a call reports back.
//
// Only [`HyphaStatus::Ok`] means an output parameter was filled.
enum HyphaStatus
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : int32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  // The call succeeded.
  HYPHA_STATUS_OK = 0,
  // A pointer was null, or a length made no sense.
  HYPHA_STATUS_INVALID_ARGUMENT = 1,
  // The topic name was empty or refused.
  HYPHA_STATUS_INVALID_TOPIC = 2,
  // No buffer was free. The caller drops this message.
  HYPHA_STATUS_POOL_EXHAUSTED = 3,
  // The subscriber holds its maximum number of received samples.
  HYPHA_STATUS_BORROW_LIMIT = 8,
  // The requested buffer is larger than the transport carries.
  HYPHA_STATUS_TOO_LARGE = 4,
  // The endpoint is closed.
  HYPHA_STATUS_DISCONNECTED = 5,
  // The transport failed for a reason of its own.
  HYPHA_STATUS_BACKEND = 6,
  // A foreign callback threw, or returned this status. The call did nothing.
  HYPHA_STATUS_PANIC = 7,
  // The received-sample frame is not an hypha frame.
  HYPHA_STATUS_INVALID_FRAME = 9,
  // Owning receive and `WaitSet` dispatch were mixed on one subscriber.
  HYPHA_STATUS_INVALID_OPERATION = 10,
  // A required service, schema record, or capability was not available.
  HYPHA_STATUS_NOT_AVAILABLE = 11,
  // A schema record failed parsing, validation, or hash checking.
  HYPHA_STATUS_INVALID_SCHEMA = 12,
  // An operation was rejected because a fixed resource limit was reached.
  HYPHA_STATUS_RESOURCE_LIMIT = 13,
  // The frame CRC-32C does not match the sample header and payload.
  HYPHA_STATUS_CORRUPT_PAYLOAD = 15,
  // The frame sequence was not newer than the last one from its publisher.
  HYPHA_STATUS_SEQUENCE_GAP = 16,
  // Static endpoint `QoS` does not match the topic service.
  HYPHA_STATUS_QOS_MISMATCH = 17,
  // Monotonic or simulation timeout elapsed.
  HYPHA_STATUS_TIMEOUT = 18,
  // Operation invoked in invalid state.
  HYPHA_STATUS_INVALID_STATE = 20,
  // Simulated time went back under a replay or triggered cycle; a running cycle never rewinds.
  HYPHA_STATUS_CLOCK_REWOUND = 22,
  // The application asked for its run to fail (`hypha_waitset_request_failure`).
  HYPHA_STATUS_APPLICATION_FAILED = 23,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum HyphaStatus HyphaStatus;
#else
typedef int32_t HyphaStatus;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

// Result of reading a parameter.
enum HyphaParamResult
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : int32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  // The parameter was present and was written to the output.
  HYPHA_PARAM_RESULT_OK = 0,
  // The parameter was not available.
  HYPHA_PARAM_RESULT_ABSENT = 1,
  // The parameter was present but is not a value the read can return: a list
  // read as a scalar, or a table element read as a scalar.
  HYPHA_PARAM_RESULT_MALFORMED = 2,
  // An input pointer was null, or a key was not UTF-8.
  HYPHA_PARAM_RESULT_INVALID_ARGUMENT = 3,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum HyphaParamResult HyphaParamResult;
#else
typedef int32_t HyphaParamResult;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

// How one cycle input binding consumes retained samples.
enum HyphaCycleInputMode
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  // Select the newest accepted sample.
  HYPHA_CYCLE_INPUT_MODE_LATEST = 0,
  // Consume retained samples with an independent cursor.
  HYPHA_CYCLE_INPUT_MODE_QUEUED = 1,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum HyphaCycleInputMode HyphaCycleInputMode;
#else
typedef uint32_t HyphaCycleInputMode;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

// Handling of one output publication refusal.
enum HyphaCycleOutputFailure
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  // Fail the owning `WaitSet` on any publication refusal.
  HYPHA_CYCLE_OUTPUT_FAILURE_FAIL_RUN = 0,
  // Drop and count only pool exhaustion; fail on every other refusal.
  HYPHA_CYCLE_OUTPUT_FAILURE_DROP_POOL_EXHAUSTED = 1,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum HyphaCycleOutputFailure HyphaCycleOutputFailure;
#else
typedef uint32_t HyphaCycleOutputFailure;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

// The severity of a line. The functions take the level as a plain `u32`, so
// an invalid value from C is counted and never undefined behaviour.
enum HyphaLogLevel
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  // Finest detail.
  HYPHA_LOG_LEVEL_TRACE = 0,
  // Detail for a developer.
  HYPHA_LOG_LEVEL_DEBUG = 1,
  // Normal events.
  HYPHA_LOG_LEVEL_INFO = 2,
  // A fault that the node survives.
  HYPHA_LOG_LEVEL_WARN = 3,
  // A fault that stops one operation.
  HYPHA_LOG_LEVEL_ERROR = 4,
  // A fault that stops the node.
  HYPHA_LOG_LEVEL_FATAL = 5,
  // No line has this level. As a threshold it passes nothing.
  HYPHA_LOG_LEVEL_OFF = 6,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum HyphaLogLevel HyphaLogLevel;
#else
typedef uint32_t HyphaLogLevel;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

// Numeric configuration constraints. A range includes both endpoints.
enum HyphaNumberConstraint
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : uint32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  // Refuse NaN and infinity.
  HYPHA_NUMBER_CONSTRAINT_FINITE = 0,
  // Require a finite value greater than zero.
  HYPHA_NUMBER_CONSTRAINT_POSITIVE = 1,
  // Require a finite value between finite, ordered bounds.
  HYPHA_NUMBER_CONSTRAINT_RANGE = 2,
  // Require a finite value of at least zero.
  HYPHA_NUMBER_CONSTRAINT_NON_NEGATIVE = 3,
  // Require a finite value less than zero.
  HYPHA_NUMBER_CONSTRAINT_NEGATIVE = 4,
  // Require a finite value of at most zero.
  HYPHA_NUMBER_CONSTRAINT_NON_POSITIVE = 5,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum HyphaNumberConstraint HyphaNumberConstraint;
#else
typedef uint32_t HyphaNumberConstraint;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

// A loaded parameter configuration, independent of any bus session. Opaque to the caller.
typedef struct HyphaConfig HyphaConfig;

// One cycle registration. Opaque to callers.
//
// The cycle owns its handle and frees it with itself, at `hypha_cycle_destroy` or the
// destruction of the `WaitSet`.
typedef struct HyphaCycle HyphaCycle;

// One callback-scoped output emission collection. Opaque to callers.
//
// Each message is one reservation (`hypha_cycle_emission_reserve`), written once
// straight into shared memory, and one commit; the library sends or releases the bytes
// after the step, and the caller never frees them. The first error of a step latches:
// the step sends nothing and the run fails. A later send failure cannot undo an earlier
// send. A `STOP` result, or `CONTINUE` with a status, sends nothing.
typedef struct HyphaCycleEmission HyphaCycleEmission;

// One callback-scoped frozen input image. Opaque to callers.
typedef struct HyphaCycleImage HyphaCycleImage;

// An owned managed input source. Opaque to callers.
typedef struct HyphaInputSource HyphaInputSource;

// A borrowed buffer. Opaque to the caller.
//
// It is released by `hypha_loan_send` or by `hypha_loan_drop`. Losing the pointer
// holds a transport buffer until the publisher is released.
typedef struct HyphaLoan HyphaLoan;

// A session on the bus. Opaque to the caller.
//
// `hypha_node_create` reads `HYPHA_USE_SIM_TIME`: `1` or `true` puts the node on
// simulated time, `0`, `false` or an unset variable leaves it off, and any other value
// is `InvalidArgument`. A refused parameter file or `[log]` table is `InvalidArgument`,
// or `InvalidState` for a sink lock another process holds or a second, different `[log]`
// table. Each `WaitSet` on the node co-owns it, so the node outlives it whatever order
// (or thread) the caller destroys them in.
typedef struct HyphaNode HyphaNode;

// A sending endpoint. Opaque to the caller.
typedef struct HyphaPublisher HyphaPublisher;

// A receiving endpoint. Opaque to the caller.
//
// It has one delivery style: owning receive (Rust SDK only) or `WaitSet` dispatch. The
// first claim fixes the style and a detach does not release it; the other style then
// returns `InvalidOperation` and consumes no sample. A notification is a wake hint, never data.
typedef struct HyphaSubscriber HyphaSubscriber;

// The transform buffer of a plain node. Opaque to the caller.
//
// It names its node weakly, so it never keeps a destroyed node alive. Any thread may
// look up through a shared reference.
typedef struct HyphaTransformBuffer HyphaTransformBuffer;

// The `WaitSet` of one node, which owns it. Opaque to the caller.
//
// One thread at a time uses it: between runs any thread, during a run the thread that
// called `hypha_waitset_run`. During a run another thread may call
// `hypha_waitset_request_stop`, `hypha_waitset_request_failure`, `hypha_cycle_health` and
// `hypha_cycle_binding`; any other call that names this `WaitSet`, one of its cycles or
// input sources returns `InvalidState` on that thread and changes nothing. On the run's
// own thread a callback may also detach a registration, applied when the run returns; an
// attach, a second run, and a call that creates, configures or destroys a cycle or an
// input source return `InvalidState`. A run must not start while another thread is
// inside a call on this `WaitSet`: that is a caller error with no status.
typedef struct HyphaWaitSet HyphaWaitSet;

// The settings of a cycle that drives `/clock` for a simulator.
typedef struct HyphaClockDriverOptions {
  // The first `/clock` value, in nanoseconds. Zero or more.
  int64_t start_nanos;
  // A required participant silent this long fails the run. Positive.
  uint64_t timeout_nanos;
  // Simulated seconds per host second. Zero steps as fast as the participants allow;
  // otherwise positive and finite.
  double real_time_factor;
} HyphaClockDriverOptions;

// Borrowed payload selected for one binding during the cycle callback.
typedef struct HyphaCycleSampleView {
  // ROS wire payload bytes.
  const uint8_t *payload;
  // Payload byte count.
  size_t payload_len;
  // Host monotonic transport send time.
  uint64_t transport_sent_nanos;
  // The sample's sequence in its source, for `hypha_cycle_image_reject_sample`.
  uint64_t sequence;
  // The publisher instance that sent the frame.
  uint64_t publisher_id;
  // The send sequence of that publisher, from the frame header. Wraps.
  uint64_t publisher_sequence;
} HyphaCycleSampleView;

// Capture result for one input binding.
typedef struct HyphaCycleBindingReport {
  // One of the `HYPHA_CYCLE_VERDICT_*` values.
  uint32_t verdict;
  // Usable samples selected for this iteration.
  size_t sample_count;
  // Queued retained samples still pending through the high-water mark. Zero for a
  // latest binding.
  size_t backlog_samples;
  // The oldest excess pending samples discarded in this step.
  uint64_t work_limit_drops;
  // Samples overwritten before this binding examined them since last capture.
  uint64_t lost_since_capture;
  // Cumulative samples lost to internal history retention for this binding.
  uint64_t lost_total;
  // Samples older than the silence limit, and late triggers, in this capture.
  uint64_t stale_since_capture;
  // Cumulative samples rejected by age for this binding.
  uint64_t stale_total;
} HyphaCycleBindingReport;

// Cumulative outcomes for one input binding. Every count wraps.
typedef struct HyphaCycleBindingHealth {
  // Most recent image report.
  struct HyphaCycleBindingReport capture;
  // Cumulative history losses and, for a queued binding, transport sequence gaps; a
  // latest binding counts neither.
  uint64_t overrun_total;
  // Cumulative samples intentionally discarded by queued work policies.
  uint64_t work_limit_drops_total;
  // Cumulative age rejections.
  uint64_t age_rejections_total;
  // Latest iterations with no usable value.
  uint64_t latest_unavailable_iterations;
  // Transitions from usable to stale latest value.
  uint64_t usable_to_stale_transitions;
} HyphaCycleBindingHealth;

// The held sample facts of one input binding: the newest usable sample it captured.
typedef struct HyphaCycleHeldReport {
  // Samples the source had accepted at capture. Wraps.
  uint64_t received_total;
  // Usable samples that became the held sample. Wraps.
  uint64_t delivered_total;
  // Nonzero when a held sample exists.
  uint8_t has_held;
  // Sequence of the held sample. Zero when `has_held` is zero.
  uint64_t held_sequence;
} HyphaCycleHeldReport;

// The supervision state of one input binding.
typedef struct HyphaSupervisionReport {
  // Bit mask of `HYPHA_FAULT_*`, also when ignored. Zero when unsupervised.
  uint32_t faults;
  // Nonzero when the fault is overridden.
  uint8_t ignored;
} HyphaSupervisionReport;

// Everything one input binding reports: in a step from the image
// (`hypha_cycle_image_binding`), or from the snapshot of the last finished step
// (`hypha_cycle_binding`, from any thread).
typedef struct HyphaCycleBindingView {
  // The cumulative counts, with the capture of the step the view describes.
  struct HyphaCycleBindingHealth health;
  // The held sample facts at that capture.
  struct HyphaCycleHeldReport held;
  // The supervision faults and the ignore flag at that capture. Zero for an
  // unsupervised binding.
  struct HyphaSupervisionReport supervision;
  // 1 when the binding is ok for its node: unwired, advisory (`optional`), ignored,
  // or usable and not stagnant.
  uint8_t ok;
  // The age the verdict compared with the silence limit, on the capture clock: of
  // the newest admitted sample, or, before any sample, the time since the first capture.
  uint64_t age_nanos;
} HyphaCycleBindingView;

// Silence and stagnation ranges of one supervised input, in nanoseconds.
//
// A limit outside its range is refused, never clamped.
typedef struct HyphaSupervisionRanges {
  // Smallest accepted `silence_ms`, in nanoseconds.
  int64_t silence_min_nanos;
  // Largest accepted `silence_ms`, in nanoseconds.
  int64_t silence_max_nanos;
  // Smallest accepted `stagnation_ms`, in nanoseconds.
  int64_t stagnation_min_nanos;
  // Largest accepted `stagnation_ms`, in nanoseconds.
  int64_t stagnation_max_nanos;
} HyphaSupervisionRanges;

// What one input's supervision binding reads: the port table, the checks the code
// It is read for the call only.
//
// `silence_ms` is required on an enabled input that counts for health unless
// `unsupervised` is set, optional on an advisory one and refused with `unsupervised`;
// `stagnation_ms` is read when `check_stagnation` is set. A key the request does not
// allow and a limit out of range are `InvalidArgument`; a declared limit that is
// missing is `NotAvailable`.
typedef struct HyphaInputSupervisionRequest {
  // The node whose parameter file holds `ports.inputs.<name>`. Not null.
  const struct HyphaNode *node;
  // The port name, `name_len` UTF-8 bytes. Empty for an unnamed port, which
  // accepts no declared check.
  const char *name;
  // The byte length of `name`.
  size_t name_len;
  // Nonzero when the port is tagged `unsupervised`: it runs without `silence_ms`.
  uint8_t unsupervised;
  // Nonzero when the port reads `stagnation_ms` (it has an alive counter).
  uint8_t check_stagnation;
  // Nonzero when the port reads its topic from the table, so a `topic` key is allowed.
  uint8_t takes_topic;
  // The bounds a limit must meet. A limit outside them is refused, never clamped.
  struct HyphaSupervisionRanges ranges;
} HyphaInputSupervisionRequest;

// Silence and stagnation limits of one supervised input, in nanoseconds.
// Zero turns that check off.
typedef struct HyphaSupervisionLimits {
  // Time without an accepted sample before the input is silent.
  int64_t silence_nanos;
  // Time without an alive change before the input is stagnant.
  int64_t stagnation_nanos;
} HyphaSupervisionLimits;

// What the binding attached.
typedef struct HyphaInputSupervisionResult {
  // Nonzero when limits attached: the input is supervised.
  uint8_t attached;
  // The table's `ignored` flag. Zero unless `attached` is nonzero.
  uint8_t ignored;
  // The limits that attached, in nanoseconds; zero when `attached` is zero.
  struct HyphaSupervisionLimits limits;
} HyphaInputSupervisionResult;

// Cumulative cycle outcomes.
typedef struct HyphaCycleHealth {
  // Completed step callbacks.
  uint64_t iterations;
  // Cumulative timer missed periods.
  uint64_t missed_periods;
  // Step and pre-dispatch failures.
  uint64_t callback_failures;
  // Output send attempts.
  uint64_t output_attempts;
  // Successful output sends.
  uint64_t output_successes;
  // Failed output sends, including explicitly dropped pool pressure.
  uint64_t output_failures;
  // Pool-exhausted outputs dropped under the diagnostic policy.
  uint64_t output_pool_drops;
  // Framework time of the latest capture.
  int64_t observed_framework_nanos;
  // Host steady time of the latest capture.
  int64_t observed_steady_nanos;
  // Step callbacks that returned, failed ones included.
  uint64_t steps;
  // Host monotonic duration of the last step callback; zero before the first.
  uint64_t last_step_nanos;
  // The longest step callback since the commit, in host monotonic time; zero before
  // the first.
  uint64_t max_step_nanos;
  // Steps longer than the period, on host time only; zero for a triggered cycle and
  // under simulated time.
  uint64_t overruns;
} HyphaCycleHealth;

// What a callback asks the run loop to do next.
typedef struct HyphaCallbackResult {
  // One of the `HYPHA_ACTION_*` values.
  uint32_t action;
  // An [`HyphaStatus`] value. Zero reports success. A nonzero status with
  // the continue action reports a failure that does not stop the run.
  int32_t status;
} HyphaCallbackResult;

// Time observed for one cycle iteration.
typedef struct HyphaCycleTime {
  // The time the timer was scheduled to fire: simulated time under simulated time, else
  // the monotonic host clock. Not a wall-clock stamp.
  int64_t scheduled_nanos;
  // Framework time at entry to the SDK callback, which every frame the step sends
  // carries. A replay step under simulated time takes its deadline.
  int64_t callback_start_nanos;
  // Host monotonic time used to evaluate binding ages.
  int64_t capture_steady_nanos;
  // Time between the previous step and this one: the monotonic clock's difference on host
  // time, the simulated clock's under simulated time.
  int64_t elapsed_nanos;
  // Periods that passed since the previous step without a step: 0 when the step is on
  // time, in a replay step and in a triggered step.
  uint64_t missed_periods;
  // Nonzero when elapsed time is present.
  uint8_t has_elapsed;
  // Header stamp of this iteration, in nanoseconds since the Unix epoch.
  // Framework time under simulated time, otherwise wall time.
  int64_t message_stamp_nanos;
  // Nonzero when a `RosTime` can hold the stamp. Zero also when the wall clock reads
  // before the Unix epoch; the step still runs, so the caller checks the flag.
  uint8_t message_stamp_valid;
} HyphaCycleTime;

// The SDK's one call for a cycle iteration.
//
// # Safety
// Must not unwind. Image and emission views are valid only for this call.
typedef struct HyphaCallbackResult (*HyphaCycleStep)(const struct HyphaCycleImage *image,
                                                     const struct HyphaCycleTime *time,
                                                     struct HyphaCycleEmission *emission,
                                                     void *context);

// Releases a callback context. A successful attach transfers ownership of the
// context to the registration when this function is supplied.
//
// # Safety
// Must not unwind. Runs exactly once, after no callback can run again.
typedef void (*HyphaContextRelease)(void *context);

// Capture policy for one cycle input binding.
typedef struct HyphaCycleBindingOptions {
  // Latest or queued capture, a `HyphaCycleInputMode`. Another value is
  // `HYPHA_STATUS_INVALID_ARGUMENT`.
  uint32_t mode;
  // Queued samples examined per iteration. Must be nonzero for queued mode. A queued
  // binding with more pending samples than its share drops the oldest of them.
  size_t max_samples_per_iteration;
  // Nonzero excludes the binding from node health. A disabled input is never judged.
  uint8_t optional;
} HyphaCycleBindingOptions;

// One looked-up transform: where `source` sits in `target`.
typedef struct HyphaTransform {
  // The source origin in the target frame.
  double translation[3];
  // The source orientation in the target frame, a unit quaternion `[x, y, z, w]`.
  double rotation_xyzw[4];
  // The time the transform holds for, in nanoseconds. A lookup of the latest
  // common time gets that time; 0 for an all-static chain.
  int64_t stamp_nanos;
} HyphaTransform;

// The identifier of a registered module.
typedef uint32_t HyphaLogModule;

// Receives one metric. `name` points to `name_len` UTF-8 bytes and is valid
// only during the call. `user_data` is the pointer passed to the reader.
typedef void (*HyphaMetricCallback)(const char *name,
                                    size_t name_len,
                                    uint64_t value,
                                    void *user_data);

// Node information reported by `hypha_list_nodes`.
typedef struct HyphaNodeInfo {
  // The node's registered name.
  const char *name;
  // The node's executable name/path.
  const char *executable;
  // Process ID.
  int32_t pid;
  // Whether the process is alive (1) or dead (0).
  int32_t alive;
} HyphaNodeInfo;

// Called once per node registered on the bus, by `hypha_list_nodes`.
typedef void (*HyphaNodeCallback)(const struct HyphaNodeInfo *info,
                                  void *user_data);

// Endpoint counts for one data service.
typedef struct HyphaServicePortInfo {
  // Topic service name.
  const char *name;
  // Active publisher count.
  size_t publishers;
  // Active subscriber count.
  size_t subscribers;
} HyphaServicePortInfo;

// Callback used by `hypha_list_service_ports`.
typedef void (*HyphaServicePortCallback)(const struct HyphaServicePortInfo *info,
                                         void *user_data);

// A configured value as it is stored. Only the field named by `kind` is
// meaningful; the others are zero or null. `text` is not NUL-terminated and
// borrows the configuration until it is destroyed (`hypha_config_destroy`, or
// `hypha_node_destroy` for a node's own configuration).
typedef struct HyphaConfigValue {
  // One of `HYPHA_CONFIG_VALUE_*`.
  uint32_t kind;
  // 0 or 1 when `kind` is `HYPHA_CONFIG_VALUE_BOOL`, else 0.
  uint8_t bool_value;
  // The whole number when `kind` is `HYPHA_CONFIG_VALUE_INT`, else 0.
  int64_t int_value;
  // The number when `kind` is `HYPHA_CONFIG_VALUE_FLOAT`, else 0.
  double float_value;
  // The bytes when `kind` is `HYPHA_CONFIG_VALUE_TEXT`, else null.
  const char *text;
  // The byte length of `text`, else 0.
  size_t text_len;
} HyphaConfigValue;

// The service policy an endpoint declares, and the payload bound of a publisher.
// The struct holds no allocation and needs no destruction. The name is UTF-8, not
// NUL-terminated, borrowed for the call that receives it and copied before that call
// returns. A refused limit or name is `HYPHA_STATUS_INVALID_ARGUMENT`. A policy declared
// on a framework topic (`/clock`, `/clock_progress`) is checked for validity only.
typedef struct HyphaQos {
  // The policy name bytes, or null when `name_len` is zero. The name is 1 to 32
  // letters, digits, `_`, `-` or `.`, starting with a letter or digit.
  //
  // `name_len == 0` declares no policy; the endpoint adopts the topic's service. A
  // publisher creates a missing service with hypha's `default` policy. A subscriber
  // gets `HYPHA_STATUS_NOT_AVAILABLE` until a publisher has opened a non-framework topic.
  //
  // `name_len > 0`: the service must carry the same name and limits, or the endpoint is
  // refused with `HYPHA_STATUS_QOS_MISMATCH`. A missing service is created with them.
  const char *name;
  // The byte count of `name`. Zero declares no policy.
  size_t name_len;
  // Publishers the policy admits: 1 to 256 (`default`: 8; a framework topic: 32).
  // Ignored when `name_len` is zero.
  uint32_t max_publishers;
  // Subscribers the policy admits: 1 to 256 (`default`: 16; a framework topic: 32).
  // Ignored when `name_len` is zero.
  uint32_t max_subscribers;
  // The deepest queue a subscriber of the policy may ask for: 1 to 1024 (`default`
  // and a framework topic: 32). Ignored when `name_len` is zero.
  uint32_t max_depth;
  // The payload bound of a publisher, in bytes, for a type whose size varies. Zero
  // means growable. A fixed-size type is bounded to its exact encoded size, and a
  // larger loan or publish than the bound is `HYPHA_STATUS_TOO_LARGE`. A subscriber
  // ignores it.
  uint64_t max_payload_dynamic;
  // The first slot size, in bytes, of a growable publisher: a hint, not a bound. Zero
  // takes the framework default (4096). A value that does not fit `size_t` is
  // `HYPHA_STATUS_INVALID_ARGUMENT`.
  uint64_t initial_payload;
} HyphaQos;

// Endpoint options of a publisher, passed to `hypha_publish_with_qos`.
//
// The payload bound is `qos.max_payload_dynamic`: zero is growable, and a fixed-size
// type is bounded to its exact encoded size. A bound that does not fit `size_t` is
// `HYPHA_STATUS_INVALID_ARGUMENT`; a larger loan or publish fails with
// `HYPHA_STATUS_TOO_LARGE` and sends nothing. A growable publisher's first slot size is
// `qos.initial_payload`, 4096 bytes when zero.
typedef struct HyphaPublisherOptions {
  // Non-zero enables late-joiner replay.
  int32_t latch;
  // Samples this publisher may hold on loan at once. Zero selects the framework
  // default, `PUBLISHER_LOANED_SAMPLES` (2). A value that does not fit is
  // `HYPHA_STATUS_RESOURCE_LIMIT`.
  uint32_t loaned_samples;
  // The service policy this publisher declares, and its payload bound.
  struct HyphaQos qos;
} HyphaPublisherOptions;

// Endpoint and resource options of a subscriber, passed to `hypha_subscribe_typed`.
//
// `qos` declares the service policy the topic must carry, or none. Its payload bound
// does not apply to a subscriber.
typedef struct HyphaSubscriberOptions {
  // Number of pending samples retained for this subscriber. Zero selects the
  // service's maximum depth. A depth above the service's `max_depth` is refused
  // with `HYPHA_STATUS_RESOURCE_LIMIT`.
  size_t depth;
  // Warn once after this many nanoseconds without a connected publisher. Zero disables it.
  // A wildcard pattern never warns.
  uint64_t no_publisher_warning_after_nanos;
  // The service policy this subscriber declares, or none.
  struct HyphaQos qos;
} HyphaSubscriberOptions;

// The counters of one subscriber, as `hypha_subscriber_counters` writes them: eight
// `uint64_t`, 64 bytes, owned by the caller, with nothing to destroy. Every counter but
// `loan_budget` and `loan_held` only grows, and wraps.
typedef struct HyphaSubscriberCounters {
  // `integrity.sequence_gap`: frames refused because their sequence was not newer
  // than the last accepted one of their publisher. A lost frame is not one.
  uint64_t sequence_gap;
  // `integrity.invalid_frame`: frames whose layout is not an hypha frame.
  uint64_t invalid_frame;
  // `integrity.corrupt_payload`: frames whose CRC-32C does not match.
  uint64_t corrupt_payload;
  // `integrity.schema_mismatch`: frames of another schema than the subscriber expects.
  uint64_t schema_mismatch;
  // `integrity.invalid_timestamp`: frames sent later than the host clock reads.
  uint64_t invalid_timestamp;
  // `loan.budget`: the owning samples the subscriber may hold at once.
  uint64_t loan_budget;
  // `loan.held`: the owning samples it holds now.
  uint64_t loan_held;
  // `loan.refused`: owning receives refused because the budget was full.
  uint64_t loan_refused;
} HyphaSubscriberCounters;

// Identity joined from the subscription topic and the frame header.
typedef struct HyphaSampleIdentity {
  // Subscription topic bytes.
  const char *topic;
  // Topic byte count.
  size_t topic_len;
  // Host monotonic time, in nanoseconds, at which the publisher sent the frame.
  uint64_t transport_sent_nanos;
  // Framework time, in nanoseconds, at which the publisher sent the frame: simulated
  // time on a sim-time node, and wall time otherwise.
  uint64_t framework_sent_nanos;
  // 64-bit schema hash prefix from MD5.
  uint64_t schema_hash;
} HyphaSampleIdentity;

// One taken sample, handed to a subscription callback.
//
// A frame that did not parse is still delivered, with `payload` null and
// `payload_len` zero; `identity.topic` is filled in regardless.
typedef struct HyphaDelivery {
  // The message payload, or null when the frame did not parse.
  const uint8_t *payload;
  // Payload byte count. Zero when `payload` is null.
  size_t payload_len;
  // Topic, and, when the frame parsed, schema hash and send time.
  struct HyphaSampleIdentity identity;
  // Nanoseconds between the send and this delivery, on the host monotonic
  // clock. Zero when `payload` is null.
  uint64_t transport_age_nanos;
} HyphaDelivery;

// Called for each sample a subscription registration accepts.
//
// # Safety
// Must not unwind. `delivery` is valid only for the call.
typedef struct HyphaCallbackResult (*HyphaSubscriptionCallback)(const struct HyphaDelivery *delivery,
                                                                void *context);

// Called once for each period a timer registration is due.
//
// # Safety
// Must not unwind.
typedef struct HyphaCallbackResult (*HyphaIntervalCallback)(uint64_t missed_periods,
                                                            void *context);

// Describes one registration. `kind` is one of the `HYPHA_ATTACH_*` values.
// Each kind reads only the fields its documentation names; every other field must
// hold its cleared value. A subscription shares its subscriber until the detach; the
// caller's `hypha_subscriber_destroy` releases only its own handle.
typedef struct HyphaAttachment {
  // One of the `HYPHA_ATTACH_*` values.
  uint32_t kind;
  // Caller context for the callback. Transferred on a successful attach.
  void *context;
  // Releases `context`. May be null when `context` needs no release.
  HyphaContextRelease release;
  // Subscription: the subscriber. The registration shares it until the detach.
  struct HyphaSubscriber *subscriber;
  // Subscription: called for each sample the subscription accepts.
  HyphaSubscriptionCallback subscription_callback;
  // Timer: the period in nanoseconds. Must be positive and at most the largest
  // framework duration (`InvalidArgument` otherwise).
  uint64_t period_nanos;
  // Timer: called once for each due period.
  HyphaIntervalCallback periodic_callback;
} HyphaAttachment;

// Identifies one registration. Never zero, and never the identifier of
// another live registration.
typedef uint64_t HyphaAttachmentId;

// The outcome of one run.
typedef struct HyphaWaitSetRunReport {
  // One of the `HYPHA_RUN_REASON_*` values.
  uint32_t reason;
  // An [`HyphaStatus`] value. Zero when the run stopped as requested.
  int32_t status;
  // The registration the failure belongs to. Zero when it belongs to none.
  HyphaAttachmentId attachment;
  // Callbacks that ran longer than 10 ms of host monotonic time. Each is logged at
  // most once per 1 s warning interval.
  uint64_t slow_callbacks;
} HyphaWaitSetRunReport;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// Returns the ABI revision compiled into this shared library.
//
// C and C++ clients must compare this exactly with `HYPHA_ABI_REVISION` before any
// other ABI call; no other call checks the caller's revision.
uint32_t hypha_abi_revision(void);

// Return framework time in nanoseconds since the Unix epoch (UTC), simulated while
// simulated time is enabled. Inside a cycle step it is the time of the step.
int64_t hypha_clock_now_nanos(void);

// Return the host monotonic clock time in nanoseconds. It never follows simulated time.
int64_t hypha_clock_steady_now_nanos(void);

// Return 1 while the active clock is simulated, else 0.
int32_t hypha_clock_is_sim_time(void);

// Enable simulated time at `initial_nanos`, for a test or a tool that owns the
// simulation; not beside a sim-time node or a clock-driver cycle.
//
// Returns `HyphaStatus::InvalidArgument` when `initial_nanos` is negative.
int32_t hypha_clock_enable_sim_time(int64_t initial_nanos);

// Disable simulated time and restore the UTC wall clock. Returns `HyphaStatus::Ok`.
int32_t hypha_clock_disable_sim_time(void);

// Set the simulated clock value.
//
// Returns `HyphaStatus::InvalidArgument` when simulated time is disabled or
// `nanos` is negative.
int32_t hypha_clock_set_sim_time(int64_t nanos);

// Advance the simulated clock by `delta_nanos`.
//
// Returns `HyphaStatus::InvalidArgument` when simulated time is disabled or
// `delta_nanos` is negative.
int32_t hypha_clock_advance_sim_time(int64_t delta_nanos);

// Load a parameter file without joining the bus; a zero `parameter_file_len` creates
// an empty configuration. On success the caller owns `*out` and releases it with
// `hypha_config_destroy`.
//
// # Safety
// `parameter_file` holds `parameter_file_len` bytes when that is nonzero. `out` points
// to one writable pointer.
HyphaStatus hypha_config_create(const char *parameter_file,
                                size_t parameter_file_len,
                                struct HyphaConfig **out);

// Release a configuration handle. Passing null is allowed and does nothing.
//
// # Safety
// `config` must come from `hypha_config_create` and must not be used afterwards.
void hypha_config_destroy(struct HyphaConfig *config);

// Borrow the configuration of `node`. The pointer lives as long as `node` and
// must not be passed to `hypha_config_destroy`. Returns null when `node` is null.
//
// # Safety
// `node` must come from `hypha_node_create`, or be null.
const struct HyphaConfig *hypha_node_config(const struct HyphaNode *node);

// Write the unread keys of `config` under `prefix` (all for a zero `prefix_len`),
// joined by `'\n'`, to `out`; `node.name` is excluded. `TooLarge` when `out_capacity`
// is too small; `out_len` then holds the byte length.
//
// # Safety
// `config` is live. `prefix` holds `prefix_len` bytes when that is nonzero. `out` is
// null or `out_capacity` writable bytes. `out_len` points to a writable `usize`.
HyphaStatus hypha_config_unread_keys(const struct HyphaConfig *config,
                                     const char *prefix,
                                     size_t prefix_len,
                                     char *out,
                                     size_t out_capacity,
                                     size_t *out_len);

// Resolve the file name that the text parameter `key` holds against the directory of
// the file that wrote `key`, and write it to `out`. An absolute name, or any name when
// `config` was loaded from no file, stays as it is. Marks `key` as read.
//
// `InvalidArgument`: a null `config` or `out_len`, or `key` names no text value or an
// empty one. `TooLarge`: `out_capacity` is too small; `out_len` then holds the byte
// length. `out` may be null only with `out_capacity` 0.
//
// # Safety
// `config` is live. `key` holds `key_len` bytes. `out` is null or `out_capacity`
// writable bytes. `out_len` points to a writable `usize`.
HyphaStatus hypha_config_resolve_key(const struct HyphaConfig *config,
                                     const char *key,
                                     size_t key_len,
                                     char *out,
                                     size_t out_capacity,
                                     size_t *out_len);

// Make an uncommitted rate cycle drive `/clock`; its period is the clock step. Commit
// enables process-wide simulated time at `start_nanos`. `options` is read for the call
// only. `InvalidArgument`: null pointer, a negative start, a zero timeout, a factor that
// is negative or not finite, a cycle without a rate, or a triggered cycle.
// `InvalidState`: committed cycle, or its `WaitSet` runs.
//
// # Safety
// `cycle` is null or a live handle; `options` is null or readable.
HyphaStatus hypha_cycle_set_clock_driver(struct HyphaCycle *cycle,
                                         const struct HyphaClockDriverOptions *options);

// Add the node `name` to the participants a driver cycle waits for before its second
// step. `name` points to `name_len` UTF-8 bytes, read for the call only.
// `InvalidArgument`: null cycle, empty or non-UTF-8 name, a cycle without
// `hypha_cycle_set_clock_driver`. `InvalidState`: committed cycle, or its `WaitSet` runs.
//
// # Safety
// `cycle` is null or a live handle; `name` is null or points to `name_len` bytes.
HyphaStatus hypha_cycle_expect_participant(struct HyphaCycle *cycle,
                                           const char *name,
                                           size_t name_len);

// Read sample `sample_index` of binding `binding_index` from the current image: a
// borrowed view, valid only during the callback. `InvalidArgument` for a null pointer or
// an unknown binding; `NotAvailable` for an index past the samples of the step.
//
// # Safety
// `image` and `out` must be callback-live pointers.
HyphaStatus hypha_cycle_image_sample(const struct HyphaCycleImage *image,
                                     uint32_t binding_index,
                                     size_t sample_index,
                                     struct HyphaCycleSampleView *out);

// Reserve a transport loan of `len` payload bytes for one message of `output_index`. On
// `HYPHA_STATUS_OK`, `*out` is exactly `len` writable bytes, owned by the library until
// `hypha_cycle_emission_commit` or the end of the step. A null `*out` with
// `HYPHA_STATUS_OK` means the message was dropped: write nothing and do not commit. An
// unknown index, a `len` above the payload limit, a message past the per-step count, an
// allocation failure or a reserve while one is open is latched and returned. A null
// `emission` or `out` is `InvalidArgument` and latches nothing.
// # Safety
// `emission` must be valid for the current callback. `out` must be writable.
HyphaStatus hypha_cycle_emission_reserve(struct HyphaCycleEmission *emission,
                                         uint32_t output_index,
                                         size_t len,
                                         uint8_t **out);

// Seal the reservation of the last `hypha_cycle_emission_reserve` for dispatch after
// the step. `HYPHA_STATUS_INVALID_STATE`, latched, when no reservation is open. A
// reservation still open when the step returns fails the step.
//
// # Safety
// `emission` must be valid for the current callback.
HyphaStatus hypha_cycle_emission_commit(struct HyphaCycleEmission *emission);

// Read the held sample of one image binding: the newest usable sample it captured. The
// payload is valid only during the callback. `NotAvailable` while `has_held` is zero;
// `InvalidArgument` for a null pointer or an unknown binding.
//
// # Safety
// `image` and `view` must be callback-live pointers.
HyphaStatus hypha_cycle_image_held(const struct HyphaCycleImage *image,
                                   uint32_t binding_index,
                                   struct HyphaCycleSampleView *view);

// Report the alive counter of the newest sample this callback read, applied after the
// step. Valid only during the step callback; the last value per binding counts.
// `InvalidArgument` for a null `image` or an unknown binding.
//
// # Safety
// `image` must be a callback-live pointer.
HyphaStatus hypha_cycle_image_alive(const struct HyphaCycleImage *image,
                                    uint32_t binding_index,
                                    uint64_t value);

// Override (nonzero `ignore`) or restore the fault of one binding; the fault bits do
// not change. Valid only during the step callback; applied after the step returns.
// `InvalidArgument` for a null `image` or an unknown binding.
//
// # Safety
// `image` must be a callback-live pointer.
HyphaStatus hypha_cycle_image_set_ignored(const struct HyphaCycleImage *image,
                                          uint32_t binding_index,
                                          uint8_t ignore);

// Fill `out` with the health, held sample and supervision of binding `binding_index`
// at this step. `InvalidArgument` for a null pointer or an index past the last binding;
// `out` is unchanged on an error.
//
// # Safety
// `image` and `out` must be callback-live pointers.
HyphaStatus hypha_cycle_image_binding(const struct HyphaCycleImage *image,
                                      uint32_t binding_index,
                                      struct HyphaCycleBindingView *out);

// Report that the sample `sequence` of binding `binding_index` did not decode.
//
// Valid only in the step callback, for a sample this capture selected. No binding
// selects it again; the binding is judged again without it and `out` receives that
// view. `InvalidArgument`: null `image` or `out`, an index past the last binding.
// `NotAvailable`: a sequence the capture did not select. Calling it twice is harmless.
//
// # Safety
// `image` and `out` must be callback-live pointers.
HyphaStatus hypha_cycle_image_reject_sample(const struct HyphaCycleImage *image,
                                            uint32_t binding_index,
                                            uint64_t sequence,
                                            struct HyphaCycleBindingView *out);

// Check, read and attach the supervision of input `binding_index` in one call.
//
// A disabled input reads its keys and attaches nothing. `out` receives what attached,
// and a refusal writes its cause to `detail` (`detail_capacity` bytes; `detail_len`
// receives the length; a short buffer returns `TooLarge`). `InvalidState`: the cycle is
// committed or its `WaitSet` runs.
// # Safety
// `cycle` is live; `request`, `out` and `detail_len` are non-null; the request's `node`
// is live and `name` holds `name_len` bytes. `detail` is null or `detail_capacity`
// writable bytes.
HyphaStatus hypha_cycle_bind_input_supervision(struct HyphaCycle *cycle,
                                               uint32_t binding_index,
                                               const struct HyphaInputSupervisionRequest *request,
                                               struct HyphaInputSupervisionResult *out,
                                               char *detail,
                                               size_t detail_capacity,
                                               size_t *detail_len);

// Read cumulative cycle health as of the last finished step. Counts wrap.
//
// # Safety
// `cycle` and `out` must be live pointers.
HyphaStatus hypha_cycle_health(const struct HyphaCycle *cycle,
                               struct HyphaCycleHealth *out);

// Fill `out` with the health, held sample and supervision of binding `binding_index`
// as of the last finished step; zero before the first step. `InvalidArgument` for a
// null pointer or an unknown binding.
//
// # Safety
// `cycle` and `out` must be live pointers.
HyphaStatus hypha_cycle_binding(const struct HyphaCycle *cycle,
                                uint32_t binding_index,
                                struct HyphaCycleBindingView *out);

// Create an uncommitted cycle registration, owned by `waitset`.
//
// `*out` is borrowed: the `WaitSet` destroys the cycle, its callback context and handle
// when it is destroyed; `hypha_cycle_destroy` releases it earlier. A refused create
// leaves `context` to the caller. `period_nanos` is at most `i64::MAX`; zero makes a
// triggered cycle, which needs `hypha_cycle_set_trigger` before `hypha_cycle_commit`.
// `InvalidArgument`: a name another cycle of the `WaitSet` holds. `InvalidState`: a
// running `WaitSet`.
//
// # Safety
// `waitset`, name bytes, callback context and `out` must be valid.
HyphaStatus hypha_cycle_create(struct HyphaWaitSet *waitset,
                               const char *name,
                               size_t name_len,
                               uint64_t period_nanos,
                               HyphaCycleStep callback,
                               void *context,
                               HyphaContextRelease release,
                               struct HyphaCycle **out);

// Bind one source to an uncommitted cycle with `options`, read for the call, and write
// its input index to `out_index` (from zero in declaration order, apart from outputs).
// The cycle keeps the source's subscriber and history until it is destroyed; the caller
// keeps and destroys the handle. `InvalidArgument`: a source of another `WaitSet` or an
// unknown mode. `InvalidState`: a committed cycle or a running `WaitSet`.
//
// # Safety
// `cycle`, `source`, `options`, and `out_index` must be live pointers.
HyphaStatus hypha_cycle_bind_input(struct HyphaCycle *cycle,
                                   struct HyphaInputSource *source,
                                   const struct HyphaCycleBindingOptions *options,
                                   uint32_t *out_index);

// Bind and take exclusive ownership of one publisher, as an output that sends at most
// `max_messages_per_step` messages a step under `policy` (`HyphaCycleOutputFailure`).
// `out_index` counts outputs from zero, apart from inputs. On `Ok` the cycle owns
// `publisher` until `hypha_cycle_destroy`: the caller must not destroy it, but may read
// it with `hypha_publisher_subscriber_count`. A refused bind leaves it to the caller.
// `InvalidArgument`: a zero count, an unknown policy, a publisher of another
// node. `ResourceLimit`: fewer loans than the count. `InvalidState`: a publisher another
// cycle owns, a committed cycle or a running `WaitSet`.
//
// # Safety
// `cycle`, `publisher`, and `out_index` must be live pointers.
HyphaStatus hypha_cycle_bind_output(struct HyphaCycle *cycle,
                                    struct HyphaPublisher *publisher,
                                    uint32_t policy,
                                    uint32_t max_messages_per_step,
                                    uint32_t *out_index);

// Make an uncommitted cycle, created with `period_nanos == 0`, run one step per sample
// of its queued, wired input `binding_index`; the trigger drops no arrival. Commit
// requires the binding's silence limit to be positive; after that limit without a
// sample a fallback step runs. `InvalidArgument`: null cycle, unknown, latest or unwired
// binding. `InvalidState`: committed cycle or running `WaitSet`.
//
// # Safety
// `cycle` is null or a live handle.
HyphaStatus hypha_cycle_set_trigger(struct HyphaCycle *cycle,
                                    uint32_t binding_index);

// Capture the latest input `binding_index` of an uncommitted cycle on the host clock,
// for data stamped on another clock. Under simulated time a step takes the newest
// sample whatever its stamp and judges its silence limit by host time.
// `InvalidArgument`: null cycle, unknown or queued binding.
// `InvalidState`: committed cycle or running `WaitSet`.
//
// # Safety
// `cycle` is null or a live handle.
HyphaStatus hypha_cycle_set_host_clock(struct HyphaCycle *cycle,
                                       uint32_t binding_index);

// Mark a class-based cycle so its `[cycle]` and input tables may override code
// defaults. The commit reads `[cycle]` keys `arrival_budget` (optional positive limit
// on queued arrivals examined in one step) and `rate_hz`; any other key refuses the
// commit. `InvalidState`: a committed cycle or a running `WaitSet`.
//
// # Safety
// `cycle` must be a live handle.
HyphaStatus hypha_cycle_set_class_based(struct HyphaCycle *cycle,
                                        uint8_t class_based);

// Atomically attach the cycle timer, trigger or clock driver after all bindings; an
// uncommitted cycle never runs. `InvalidArgument`: neither a period nor a trigger, both,
// a trigger binding without a positive silence limit, or a refused `[cycle]` key.
// `InvalidState`: a committed cycle, a running `WaitSet`, any cycle on a CLI tool node
// or on a node with a clock driver, and a driver in a process already on simulated
// time, on a node with another cycle, or while another process publishes `/clock`. A
// driver commit enables simulated time. A refusal reason goes to `hypha_last_error`.
// # Safety
// `cycle` must be a live handle.
HyphaStatus hypha_cycle_commit(struct HyphaCycle *cycle);

// Destroy a cycle before its `WaitSet`: release its callback context, publishers and
// source references and free the handle. A running `WaitSet` refuses with
// `InvalidState` and changes nothing. A destroyed `WaitSet` destroys its cycles.
// Destroying a clock driver, or its `WaitSet`, turns process simulated time off.
//
// # Safety
// `cycle` must be a live handle.
HyphaStatus hypha_cycle_destroy(struct HyphaCycle *cycle);

// Open a managed receiver that stores accepted message bytes in bounded shared
// history. Success transfers `subscriber`; failure leaves it to the caller. A null
// `subscriber` makes an unwired input, which captures as `NeverArrived`.
//
// `InvalidArgument`: null `waitset` or `out`, or a subscriber of another node.
// `InvalidOperation`: the subscriber is attached to a `WaitSet`, was read by an owning
// receive, or belongs to another input source. `InvalidState`: the `WaitSet` runs.
// `ResourceLimit`: no room for another source. The caller destroys the handle with
// `hypha_input_source_destroy`.
//
// # Safety
// `waitset` must be live, `subscriber` null or live, and `out` writable.
HyphaStatus hypha_input_source_create(struct HyphaWaitSet *waitset,
                                      struct HyphaSubscriber *subscriber,
                                      struct HyphaInputSource **out);

// Release the source handle. A cycle that binds the source keeps its subscriber and
// history alive until that cycle is destroyed. A running `WaitSet` refuses with
// `InvalidState` and keeps the handle valid.
//
// # Safety
// `source` must be a live source handle.
HyphaStatus hypha_input_source_destroy(struct HyphaInputSource *source);

// Bind a TF input to an uncommitted cycle over two different managed
// `tf2_msgs/TFMessage` sources of its `WaitSet`; the caller still destroys both handles.
// `cache_nanos` > 0 is the history per frame. `*out_index` (0 on a refusal) counts TF
// inputs from zero per cycle. `InvalidArgument`: null, zero cache, the same source
// twice, an unwired or foreign source. `InvalidState`: committed cycle or running `WaitSet`.
//
// # Safety
// `cycle`, `tf`, `tf_static` and `out_index` are null or live.
HyphaStatus hypha_cycle_bind_transforms(struct HyphaCycle *cycle,
                                        struct HyphaInputSource *tf,
                                        struct HyphaInputSource *tf_static,
                                        uint64_t cache_nanos,
                                        uint32_t *out_index);

// Look up, in a step, `source` in `target` at `at_nanos`, or with `latest` nonzero at the
// latest time common to the chain (`at_nanos` must then be 0). Names are UTF-8 of the
// given length and lose one leading `/`; equal names give the identity. On `Ok`,
// `*out_result` is a `HYPHA_TF_*` value; `*out` is written only for `HYPHA_TF_FOUND`.
// `InvalidArgument`: null, unknown index, non-UTF-8 names, `latest` with a nonzero
// `at_nanos`. `InvalidState`: an overflow composing the chain.
//
// # Safety
// `image` is null or the step's live image. `target` and `source` are null or hold
// their byte lengths. `out_result` and `out` are null or writable.
HyphaStatus hypha_cycle_image_lookup_transform(const struct HyphaCycleImage *image,
                                               uint32_t index,
                                               const char *target,
                                               size_t target_len,
                                               const char *source,
                                               size_t source_len,
                                               int64_t at_nanos,
                                               uint32_t latest,
                                               uint32_t *out_result,
                                               struct HyphaTransform *out);

// Record one configuration row of `node`; a repeated `key` replaces its row. A null
// `default_text` means no default. `source` is a `HYPHA_CONFIG_SOURCE_*` value.
//
// # Safety
// `node` must be a live node. `key` and `value_text` must be NUL-terminated
// strings. `default_text` must be null or a NUL-terminated string.
HyphaStatus hypha_node_record_config(struct HyphaNode *node,
                                     const char *key,
                                     const char *value_text,
                                     const char *default_text,
                                     uint32_t source);

// Record a configuration row before node creation, copied into each node created from
// `config`. As `hypha_node_record_config` otherwise.
//
// # Safety
// `config` must be live. `key`, `value_text` and a non-null `default_text` must
// be NUL-terminated UTF-8 strings.
HyphaStatus hypha_config_record_config(const struct HyphaConfig *config,
                                       const char *key,
                                       const char *value_text,
                                       const char *default_text,
                                       uint32_t source);

// Copy a snapshot of `node`, taken now, into `out` as a ROS 1 serialized
// `hypha_msgs/NodeIntrospection`.
//
// `out_len` receives the snapshot length. When `capacity` is smaller, nothing is copied
// and `TooLarge` is returned; a retry takes a new snapshot, which may have grown. `out`
// may be null only when `capacity` is 0. The caller owns `out`.
// # Safety
// `node` must be a live node. `out` must be null or point to `capacity`
// writable bytes. `out_len` must point to a writable `size_t`.
HyphaStatus hypha_node_introspect(const struct HyphaNode *node,
                                  uint8_t *out,
                                  size_t capacity,
                                  size_t *out_len);

// Copy the reason of the last refusal on this thread to `out`, UTF-8 without a
// terminator; empty when none was recorded since the last `hypha_cycle_create` or
// `hypha_cycle_commit` on this thread. `*out_len` receives the length; `TooLarge`, with
// `out` untouched, when `capacity` is too small. `InvalidArgument` for a null `out_len`.
//
// # Safety
// `out` is null or `capacity` writable bytes. `out_len` points to a writable `size_t`.
HyphaStatus hypha_last_error(char *out,
                             size_t capacity,
                             size_t *out_len);

// Borrow a payload buffer of `payload_size` bytes for one sample.
//
// On `Ok`, `*out` is the loan and `*out_payload` points to exactly `payload_size`
// writable bytes, valid until the loan is sent or dropped; both are null on any other
// status. The caller owns the loan and releases it with `hypha_loan_send` or
// `hypha_loan_drop`. A null `publisher`, `out` or `out_payload` is `InvalidArgument`.
//
// # Safety
// `publisher` must come from `hypha_publish_with_qos`. `out` and `out_payload` each
// point to one writable pointer.
HyphaStatus hypha_publisher_loan_frame(struct HyphaPublisher *publisher,
                                       size_t payload_size,
                                       struct HyphaLoan **out,
                                       uint8_t **out_payload);

// Send a filled loan. The loan is released whatever the result.
//
// The framework send time is `framework_sent_nanos` when it is zero or more, else the
// framework clock. A null `loan` returns `InvalidArgument`.
//
// # Safety
// `loan` must come from `hypha_publisher_loan_frame` and must not be used afterwards.
HyphaStatus hypha_loan_send(struct HyphaLoan *loan,
                            int64_t framework_sent_nanos);

// Release a loan without sending it, as when serialisation fails. Null is allowed.
//
// # Safety
// `loan` must come from `hypha_publisher_loan_frame` and must not be used afterwards.
void hypha_loan_drop(struct HyphaLoan *loan);

// Register a module by name; the same name gives the same identifier. Once 256 modules
// (`default` included) exist, a new name gives `HYPHA_STATUS_OK` and the identifier of
// `default` (0). A name has 1 to 64 bytes from `A-Z`, `a-z`, `0-9`, `_` and `/`.
// `InvalidArgument` for a null `out` or an invalid name.
//
// # Safety
// `name` holds `name_len` bytes. `out` points to writable storage for one identifier.
HyphaStatus hypha_log_module_register(const char *name,
                                      size_t name_len,
                                      HyphaLogModule *out);

// One when a line at `level` (a [`HyphaLogLevel`] value) reaches at least one target,
// otherwise zero, as for an unknown module or an invalid level. Lock-free.
int32_t hypha_log_enabled(HyphaLogModule module,
                          uint32_t level);

// Write one line at `level`, a [`HyphaLogLevel`] value. No status: a rejected line (an
// unknown module, an invalid level, the level off, or a null `text` with a length above
// zero) is counted. The text need not be UTF-8; a message above 16 KiB is cut.
//
// # Safety
// `text` must be null, or point to `text_len` readable bytes.
void hypha_log_write(HyphaLogModule module,
                     uint32_t level,
                     const char *text,
                     size_t text_len);

// Emit the metrics of `node`, once each, before the call returns. A null `node` emits
// only the process-wide log counters. A null `callback` returns `InvalidArgument` and
// calls nothing. `user_data` is passed to every callback unchanged and may be null;
// the library keeps no copy of either.
//
// # Safety
// `node` must be null or come from `hypha_node_create`. `callback` must be
// safe to call from this thread with `user_data`.
HyphaStatus hypha_node_metrics(const struct HyphaNode *node,
                               HyphaMetricCallback callback,
                               void *user_data);

// Write the name of `node` (`node.name` when the parameter file sets it) to `out`.
// `TooLarge` when `capacity` is too small; `out_len` then holds the length, and no
// byte is written.
//
// # Safety
// `node` is live. `out` is null or `capacity` writable bytes. `out_len` points to a
// writable `size_t`.
HyphaStatus hypha_node_name(const struct HyphaNode *node,
                            char *out,
                            size_t capacity,
                            size_t *out_len);

// Create a node called `name`: `[a-z][a-z0-9_]`, at most 64 bytes, that no live node of
// another process in the domain holds (`InvalidState`), or a CLI tool name `_hyphacli_`
// plus a node name (64 bytes in all), which any number of processes may hold. A
// non-null `config` (still owned by the caller) supplies the parameters; a nonzero
// `parameter_file_len` must then name its file (`InvalidArgument` otherwise). A null
// `config` loads `parameter_file`, or none when the length is 0. `Backend`: the dead
// run of its own name cannot be reclaimed.
// # Safety
// `name` points to `name_len` bytes, `parameter_file` to `parameter_file_len` bytes when
// nonzero; `config` is null or live; `out` points to one writable pointer. The caller
// releases the node with `hypha_node_destroy`.
HyphaStatus hypha_node_create(const char *name,
                              size_t name_len,
                              const char *parameter_file,
                              size_t parameter_file_len,
                              const struct HyphaConfig *config,
                              struct HyphaNode **out);

// Release a node. Passing null is allowed and does nothing.
//
// # Safety
// `node` must come from `hypha_node_create` and must not be used afterwards.
// Every endpoint made from it must be released first.
void hypha_node_destroy(struct HyphaNode *node);

// Non-zero while the node has not been asked to stop, by `hypha_node_shutdown` or by
// SIGINT or SIGTERM, which stop every node of the process for good.
//
// # Safety
// `node` must come from `hypha_node_create`, or be null.
int32_t hypha_node_ok(const struct HyphaNode *node);

// Ask the node to stop: `hypha_node_ok` then returns 0 for it. Null does nothing.
//
// # Safety
// `node` must come from `hypha_node_create`, or be null.
void hypha_node_shutdown(struct HyphaNode *node);

// List every node registered on the bus: `callback` runs once for each, before the
// call returns. `InvalidArgument`: a null `callback`. `Backend`: the node registry
// cannot be inspected.
//
// # Safety
// `user_data` is passed through to `callback` unexamined.
HyphaStatus hypha_list_nodes(HyphaNodeCallback callback,
                             void *user_data);

// Checks that `topic` is a canonical public topic name. `InvalidTopic`: a name the
// transport would refuse. `InvalidArgument`: a null or non-UTF-8 input.
//
// # Safety
// `topic` must point to `topic_len` readable bytes.
HyphaStatus hypha_topic_validate_public_name(const char *topic,
                                             size_t topic_len);

// List every data service and its active endpoint counts. A topic is reported without
// its leading `/`. `InvalidFrame`: a service name could not be reported, so the listing
// is incomplete.
//
// # Safety
// `node` must remain valid for the call. `user_data` is passed through to `callback`
// unexamined and may be null.
HyphaStatus hypha_list_service_ports(const struct HyphaNode *node,
                                     HyphaServicePortCallback callback,
                                     void *user_data);

// Judge the required inputs of `node` now and copy its state into `out` as a ROS 1
// serialized `hypha_msgs/NodeState`. It logs and changes nothing.
//
// `out_len` receives the length. When `capacity` is smaller, nothing is copied and
// `TooLarge` is returned. `out` may be null only when `capacity` is 0. The caller owns
// `out`.
//
// # Safety
// `node` is live. `out` is null or `capacity` writable bytes. `out_len` points to a
// writable `size_t`. Call it on the thread that runs the node, or while none does.
HyphaStatus hypha_node_state(const struct HyphaNode *node,
                             uint8_t *out,
                             size_t capacity,
                             size_t *out_len);

// Read the topic of port `name`, as written, from the parameter file of `node` into
// `out`, and record it as a configuration row. `direction` is a `HYPHA_PORT_*` value.
// `rule` `HYPHA_PORT_TOPIC_REQUIRED` refuses an unset topic with `NotAvailable` and
// `topic = ""` with `InvalidArgument`; `HYPHA_PORT_TOPIC_OPTIONAL` gives length 0 for
// both. `InvalidArgument`: another direction or rule, an empty name or an entry that is
// not a table with a text `topic`. `InvalidTopic`: an invalid topic. `TooLarge`: a
// short buffer.
//
// # Safety
// `node` is live. `name` holds `name_len` bytes. `out` is null or `out_capacity`
// writable bytes. `out_len` points to a writable `size_t`.
HyphaStatus hypha_param_topic(const struct HyphaNode *node,
                              uint32_t direction,
                              const char *name,
                              size_t name_len,
                              uint32_t rule,
                              char *out,
                              size_t out_capacity,
                              size_t *out_len);

// Validate a numeric value against a `HyphaNumberConstraint` (any other value:
// `InvalidArgument`). Bounds are read only for `Range`, inclusive. NaN and infinity
// are always refused.
HyphaStatus hypha_param_validate_number(double value,
                                        uint32_t constraint,
                                        double minimum,
                                        double maximum);

// Require at least one byte of UTF-8 text: `InvalidArgument` for a null pointer, a
// zero length or text that is not UTF-8, else `Ok`.
//
// # Safety
// `value` must point to `value_len` readable bytes.
HyphaStatus hypha_param_validate_nonempty(const char *value,
                                          size_t value_len);

// Read the scalar parameter `name` of `config` into `out`, unconverted.
//
// `Absent`: no scalar has that name. `Malformed`: `name` is a list. `InvalidArgument`:
// a null pointer or non-UTF-8 `name`. `out` is written only on `Ok`; a text value
// borrows `config`.
//
// # Safety
// `config` is live. `name` holds `name_len` bytes. `out` points to one writable
// `HyphaConfigValue`.
HyphaParamResult hypha_config_value(const struct HyphaConfig *config,
                                    const char *name,
                                    size_t name_len,
                                    struct HyphaConfigValue *out);

// Write the number of elements of the list `name` in `config` to `out_len`.
// `Absent`: no list has that name. `Malformed`: a scalar. `InvalidArgument`: a null
// pointer or non-UTF-8 `name`. `out_len` is written only on `Ok`.
//
// # Safety
// `config` is live. `name` holds `name_len` bytes. `out_len` points to a writable `usize`.
HyphaParamResult hypha_config_list_len(const struct HyphaConfig *config,
                                       const char *name,
                                       size_t name_len,
                                       size_t *out_len);

// Read element `index` of the list `name` in `config`, or its `field` when the element
// is a table (empty `field`: the element itself), into `out`. `Absent`: no such list,
// index or field. `Malformed`: a table with an empty `field`. `InvalidArgument`: a null
// pointer or a non-UTF-8 name. `out` is written only on `Ok`; a text value borrows `config`.
//
// # Safety
// `config` is live. `name` and `field` hold `name_len` and `field_len` bytes. `out`
// points to one writable `HyphaConfigValue`.
HyphaParamResult hypha_config_list_value(const struct HyphaConfig *config,
                                         const char *name,
                                         size_t name_len,
                                         size_t index,
                                         const char *field,
                                         size_t field_len,
                                         struct HyphaConfigValue *out);

// Publish `topic` with the endpoint `options`.
//
// With a non-zero `options.latch`, a late subscriber still receives the most recent
// samples. `InvalidTopic`: a `definition` that does not match `md5`, or a `~/` topic on
// a CLI tool node. `InvalidArgument`: a null `node`, `options` or `out`.
// # Safety
// `topic`, `data_type`, `md5` and `definition` point to their lengths of bytes.
// `options` points to an initialized [`HyphaPublisherOptions`]. The caller releases
// the publisher with `hypha_publisher_destroy`.
HyphaStatus hypha_publish_with_qos(struct HyphaNode *node,
                                   const char *topic,
                                   size_t topic_len,
                                   const char *data_type,
                                   size_t data_type_len,
                                   const char *md5,
                                   size_t md5_len,
                                   const char *definition,
                                   size_t definition_len,
                                   const struct HyphaPublisherOptions *options,
                                   struct HyphaPublisher **out);

// Release a publisher. Passing null is allowed and does nothing.
//
// # Safety
// `publisher` must come from `hypha_publish_with_qos` and must not be used afterwards.
// Every loan taken from it must be released first.
void hypha_publisher_destroy(struct HyphaPublisher *publisher);

// Writes the number of subscribers connected to the publisher's topic and returns 1.
// Returns 0, with `count` unmodified, when an argument is null.
//
// # Safety
// `publisher` must come from `hypha_publish_with_qos`, or be null.
int32_t hypha_publisher_subscriber_count(const struct HyphaPublisher *publisher,
                                         size_t *count);

// Write the smallest queue size among the subscribers connected to `publisher` into
// `out`, or 0 when none is connected. `InvalidArgument`, with `out` unwritten, when
// `publisher` or `out` is null.
//
// # Safety
// `publisher` is live or null. `out` points to a writable `size_t`.
HyphaStatus hypha_publisher_min_subscriber_buffer(const struct HyphaPublisher *publisher,
                                                  size_t *out);

// Resolves the message definition of `(topic, data_type, md5)` into `definition`, with
// no trailing NUL. A null `definition` with zero capacity writes the size to
// `definition_len` and copies nothing; a short buffer returns `TooLarge`.
// `NotAvailable`: a missing schema. `InvalidSchema`: a mismatched or invalid one.
// `InvalidState`: called from a dispatcher callback, unless the node already holds the
// definition, since it waits for the publisher's answer.
// # Safety
// All pointers must be valid for their specified lengths.
HyphaStatus hypha_schema_lookup(const struct HyphaNode *node,
                                const char *topic,
                                size_t topic_len,
                                const char *data_type,
                                size_t data_type_len,
                                const char *md5,
                                size_t md5_len,
                                char *definition,
                                size_t definition_capacity,
                                size_t *definition_len);

// Subscribe to `topic` with `options` and record `data_type` (null: empty) in the node
// introspection. `definition` (length 0: none) is the definition of `expected_md5`
// (length 0: none); with one, a non-empty `data_type` and a valid `expected_md5` are
// required (`InvalidSchema` otherwise). `InvalidArgument`: a null `node`, `options` or
// `out`. `InvalidTopic`: a `~/` topic on a CLI tool node.
// # Safety
// `topic`, `data_type`, `expected_md5` and `definition` point to their lengths of bytes.
// `options` points to an initialized [`HyphaSubscriberOptions`]. The caller releases the
// subscriber with `hypha_subscriber_destroy`.
HyphaStatus hypha_subscribe_typed(struct HyphaNode *node,
                                  const char *topic,
                                  size_t topic_len,
                                  const char *data_type,
                                  size_t data_type_len,
                                  const char *expected_md5,
                                  size_t expected_md5_len,
                                  const char *definition,
                                  size_t definition_len,
                                  const struct HyphaSubscriberOptions *options,
                                  struct HyphaSubscriber **out);

// Write the number of publishers currently connected to the subscriber's topic to
// `out`. `InvalidArgument`, with `out` unwritten, when `subscriber` or `out` is null.
//
// # Safety
// `subscriber` is null or live. `out` is null or one writable `size_t`.
HyphaStatus hypha_subscriber_publisher_count(const struct HyphaSubscriber *subscriber,
                                             size_t *out);

// Write the eight counters of `subscriber` to `*out`, the values of the subscriber
// metrics of the same names. Each is read atomically, the eight not at one instant. Any
// thread may call it. `InvalidArgument`, with `*out` unwritten, when `subscriber` or
// `out` is null.
//
// # Safety
// `subscriber` is null or live for the call. `out` is null or one writable
// `HyphaSubscriberCounters`.
HyphaStatus hypha_subscriber_counters(const struct HyphaSubscriber *subscriber,
                                      struct HyphaSubscriberCounters *out);

// Write the cumulative count of frames this subscriber lost before it read them (the
// sequence values skipped between accepted frames, per publisher) to `out`.
// `InvalidArgument`, with `out` unwritten, when `subscriber` or `out` is null.
//
// # Safety
// `subscriber` is null or live. `out` is null or one writable `uint64_t`.
HyphaStatus hypha_subscriber_missing_sequences(const struct HyphaSubscriber *subscriber,
                                               uint64_t *out);

// Write the topic `subscriber` resolved at creation, and its length to `out_len`.
// `TooLarge` when `capacity` is too small; `out_len` then holds the length.
//
// # Safety
// `subscriber` is null or live. `out` is null or `capacity` writable bytes. `out_len`
// points to a writable `size_t`.
HyphaStatus hypha_subscriber_topic(const struct HyphaSubscriber *subscriber,
                                   char *out,
                                   size_t capacity,
                                   size_t *out_len);

// Release the caller's subscriber handle; a `WaitSet` registration that shares it keeps
// it alive until the detach. Null does nothing.
//
// # Safety
// `subscriber` must come from `hypha_subscribe_typed` and must not be used afterwards.
void hypha_subscriber_destroy(struct HyphaSubscriber *subscriber);

// Create a transform buffer of `node` over the topics `tf_topic` (dynamic) and
// `tf_static_topic` (static), UTF-8 of the given lengths, borrowed for the call. Both
// are subscribed as `tf2_msgs/TFMessage` with `qos` (read for the call; payload fields
// ignored) and attached to the node's default `WaitSet`, whose runs ingest them.
// `cache_nanos` > 0 is the dynamic history kept per frame. A message that does not
// decode is counted and logged and fails nothing.
//
// `*out` (null on a refusal) is owned by the caller, released with
// `hypha_transform_buffer_destroy`. `InvalidArgument`: null `node`, `qos` or `out`,
// an empty or non-UTF-8 topic, zero `cache_nanos`, two names of one topic, an invalid
// policy. `InvalidState`: the default `WaitSet` runs. `QosMismatch`: a topic exists
// with another policy. A refused create leaves nothing attached. Call it on the thread
// that configures the node.
//
// # Safety
// Each pointer is null or valid: topics hold their byte lengths, `qos` is initialized
// with `name_len` name bytes, `out` is one writable pointer.
HyphaStatus hypha_transform_buffer_create(struct HyphaNode *node,
                                          const char *tf_topic,
                                          size_t tf_topic_len,
                                          const char *tf_static_topic,
                                          size_t tf_static_topic_len,
                                          const struct HyphaQos *qos,
                                          uint64_t cache_nanos,
                                          struct HyphaTransformBuffer **out);

// Look up, in `buffer`, `source` in `target` at `at_nanos`, or with `latest` nonzero at
// the latest time common to the chain (`at_nanos` must then be 0). Names are UTF-8 of
// the given length, borrowed for the call. On `Ok`, `*out_result` is a `HYPHA_TF_*`
// value; `*out` is written only for `HYPHA_TF_FOUND`. Semantics are those of
// `hypha_cycle_image_lookup_transform`.
//
// Any thread may call it while the handle is live, concurrently with ingestion and
// other lookups. `InvalidArgument`: null `buffer`, `target`, `source`, `out_result` or
// `out`, non-UTF-8 names, `latest` with a nonzero `at_nanos`; no output is written then.
// `InvalidState`: an overflow composing the chain.
//
// # Safety
// `buffer` is null or a live handle no thread destroys during the call. `target` and
// `source` are null or hold their byte lengths. `out_result` and `out` are null or writable.
HyphaStatus hypha_transform_buffer_lookup(const struct HyphaTransformBuffer *buffer,
                                          const char *target,
                                          size_t target_len,
                                          const char *source,
                                          size_t source_len,
                                          int64_t at_nanos,
                                          uint32_t latest,
                                          uint32_t *out_result,
                                          struct HyphaTransform *out);

// Destroy `buffer`: detach both subscribers from the node's default `WaitSet` and free
// the handle. Null does nothing. From a callback of the running `WaitSet` the detach is
// deferred to the end of the run. After `hypha_node_destroy` it only frees the handle.
//
// `InvalidState`: the `WaitSet` runs on another thread; nothing changes and the handle
// stays live. Never call it during a lookup on the same handle.
//
// # Safety
// `buffer` is null or a live handle from `hypha_transform_buffer_create`, not used
// after an `Ok`.
HyphaStatus hypha_transform_buffer_destroy(struct HyphaTransformBuffer *buffer);

// Return this node's `WaitSet`, creating it on first use.
//
// The node owns it and it stays valid as long as the node; `hypha_node_destroy`
// releases it with its cycles and every registration context.
//
// # Safety
// `node` must be a live node handle. `out` must be writable.
HyphaStatus hypha_node_default_waitset(struct HyphaNode *node,
                                       struct HyphaWaitSet **out);

// Attach a registration of any kind (see `HyphaAttachment`) and write its id to `out`.
//
// `InvalidArgument`: a `kind` other than `HYPHA_ATTACH_SUBSCRIPTION` or
// `HYPHA_ATTACH_TIMER`, a missing callback, or an uncleared field the kind does not
// read. Success transfers `context` and `release`, released once at the detach or the
// node's destruction; a refused attach transfers nothing. `InvalidState`: during a run,
// or a subscriber another registration dispatches.
// # Safety
// `waitset` and `attachment` are live; `out` is writable. The subscriber of
// `attachment` is null or a live handle from `hypha_subscribe_typed`.
HyphaStatus hypha_waitset_attach(struct HyphaWaitSet *waitset,
                                 const struct HyphaAttachment *attachment,
                                 HyphaAttachmentId *out);

// Remove a registration and release its context.
//
// From a callback of this `WaitSet` the registration is never dispatched again, and its
// context is released when the run returns. From another thread during a run:
// `InvalidState`. An unknown registration: `InvalidArgument`.
//
// # Safety
// `waitset` must be a live handle.
HyphaStatus hypha_waitset_detach(struct HyphaWaitSet *waitset,
                                 HyphaAttachmentId attachment);

// Run on the calling thread until a stop or a failure, or for at most `limit_nanos` of
// host time when it is nonzero (reason `HYPHA_RUN_REASON_DEADLINE`, status `Ok`). A
// second run of the `WaitSet` returns `InvalidState` and fills no report. A completed
// invocation returns [`HyphaStatus::Ok`] and fills the report, even when the run itself
// failed; a nonzero report status is a run failure.
//
// # Safety
// `waitset` is live. `out_report` points to one writable [`HyphaWaitSetRunReport`].
HyphaStatus hypha_waitset_run(struct HyphaWaitSet *waitset,
                              uint64_t limit_nanos,
                              struct HyphaWaitSetRunReport *out_report);

// Ask a `WaitSet` to stop. Idempotent. With no run in progress it returns
// `InvalidState` and still records the stop, so the next run returns at once. Null:
// `InvalidArgument`.
//
// # Safety
// `waitset` must be a live handle. Any thread may call it on a running `WaitSet`.
HyphaStatus hypha_waitset_request_stop(struct HyphaWaitSet *waitset);

// Ask the run of a `WaitSet` to end as failed with [`HyphaStatus::ApplicationFailed`].
//
// Requests the stop as [`hypha_waitset_request_stop`] does, with its return values.
// The run that ends while it is recorded reports `HYPHA_RUN_REASON_RUNTIME_FAILED` with
// this status and clears it; recorded before a run, that run fails at once. A cycle
// step that returns while it is recorded sends none of its staged messages; outputs
// sent before a request from another thread stay sent.
// # Safety
// `waitset` must be a live handle or null. Any thread may call it on a running
// `WaitSet`.
HyphaStatus hypha_waitset_request_failure(struct HyphaWaitSet *waitset);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* HYPHA_FFI_H */
