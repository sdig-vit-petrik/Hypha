# Sensor pipeline: an event-driven `ApplicationNode`, and a replay check

Two programs and one policy header, in the shape of a ported ROS 1 LiDAR node:

- `hypha_sensor_source` is a `hypha::ApplicationNode` that publishes a synthetic IMU, LiDAR
  and odometry stream.
- `hypha_cloud_processor` is a `hypha::ApplicationNode` (no cycle). It subscribes to the IMU
  and the point clouds, runs one timer, hands each cloud to one worker thread, looks the
  transform up at the cloud's stamp from that worker, and publishes the centroid of the
  cloud in the `odom` frame from that worker.
- `inc/sensor_pipeline_qos.h` holds the policies of both sensor topics. Both programs
  include it.

Both programs are started by `hypha::runNode`, so each publishes `~/introspection` and
`~/state` and exits 78 for a missing or malformed parameter file or a missing `[ports.*]`
table; `startup_exit_code.cmake` checks the exit code. `sensor_pipeline_compare` and
`replay_check.sh` are the replay recipe: they replay a recorded fixture into the processor
on simulated time and compare two outputs by header stamp. The porting rules behind the code are in
[ros1-migration-guide.md §10](../../docs/ros1-migration-guide.md#10-porting-an-event-driven-node-an-applicationnode).

| File | Holds |
|---|---|
| `src/sensor_source.cpp` | the source: one timer at `tick_hz` publishes every output |
| `src/cloud_processor.cpp` | the processor: run() thread, hand-over queue, worker |
| `startup_exit_code.cmake` | the start-up check: a missing or malformed parameter file, or no `[ports.*]` table, gives exit 78; ctest runs it for each node |
| `src/compare_by_stamp.cpp` | the comparison of two recordings by header stamp |
| `inc/sensor_pipeline_qos.h` | the `imu` and `lidar_cloud` policies, mapped with `hypha::QosOf<M>` |
| `config/source.toml`, `config/processor.toml` | the live parameters |
| `config/source_fixture.toml` | the source as it recorded the fixture |
| `config/processor_slow.toml` | the processor in slow mode |
| `fixture/sensor_inputs.mcap` | the recorded inputs, 98296 bytes |
| `replay_check.sh` | the recipe; ctest runs it as `sensor_pipeline_replay_check` |

## The numbers

| Stream | Live (`source.toml`) | Fixture (`source_fixture.toml`) |
|---|---|---|
| `/imu`, `sensor_msgs/Imu` | 400 Hz | 200 Hz |
| `/tf`, odom to base_link | 100 Hz | 100 Hz |
| `/lidar/points`, `sensor_msgs/PointCloud2` | 10 Hz, 16 x 1024 points, 262144 bytes of data | 10 Hz, 8 x 16 points, 2048 bytes of data |
| `/tf_static`, base_link to lidar and imu | once, latched | once, latched |

A point is four `float32` fields: x, y, z, intensity (16 bytes). A cloud's header stamp
equals the stamp of the `/tf` message the source sends just before it, in the same tick.

## Build

`./build.sh --tests` builds the example in the repository's CMake tree (it is part of
`-DHYPHA_BUILD_EXAMPLES=ON`, the default). The programs are in
`build/examples/sensor_pipeline/`, and the tools in `target/release/hypha`.
Against an installed SDK the example builds as the others do:

```sh
cmake -S examples -B build/examples -DCMAKE_PREFIX_PATH=/path/to/installed/hypha
cmake --build build/examples
```

`sensor_pipeline_compare` links `hypha::testing_mcap`, the test-only MCAP reader; an SDK
built with `-DHYPHA_BUILD_TESTING_MCAP=OFF` has none, and then only the two programs build.

## Run it live

From the repository root, in two terminals with the same `HYPHA_DOMAIN`:

```sh
HYPHA_DOMAIN=demo build/examples/sensor_pipeline/hypha_sensor_source examples/sensor_pipeline/config/source.toml
HYPHA_DOMAIN=demo build/examples/sensor_pipeline/hypha_cloud_processor examples/sensor_pipeline/config/processor.toml
```

Either may start first: both declare the same policies. `Ctrl-C` (SIGINT) or SIGTERM
stops each one; the processor then prints its `final` lines. A parameter key that nothing
read stops the processor with exit code 78 (`unread parameter(s): <key>`), as does a
missing or malformed parameter file, and a second argument stops it with exit code 2.

## Threads and ownership

`Node::run()` calls every callback on the thread that calls it, and the framework creates
no thread ([execution-model.md §3](../../docs/execution-model.md#3-the-framework-creates-no-thread)).
The processor has two threads:

| Thread | Runs |
|---|---|
| the run() thread (`runNode`) | `ImuCounter::onImu`, `CloudForwarder::onCloud`, `Reporter::onTick`; it fills the `TransformBuffer` from `/tf` and `/tf_static` |
| the worker (`CloudWorker`) | takes a cloud from the hand-over queue, computes its centroid, `TransformBuffer::lookup()` at the cloud's stamp, `Publisher::publish()` of the result |

The rules the headers give, and how the code follows them:

- `publisher.h`: "Threading: publish() runs on one thread at a time." `setup()` creates
  the result publisher before the run, and `started()` moves it into the worker. The worker
  owns it; no other thread touches it, so it needs no lock.
- `transform_buffer.h`: "lookup() and lookupLatest() may be called from any thread at any
  time while the object lives". The buffer is created before the run and destroyed after
  it, on the main thread, as the same comment requires.
- `subscriber.h`: `missingSequences()` and `metrics()`: "Any thread may read". The
  Reporter reads them on the run() thread.
- `node.h`: `requestStop()` "is the one operation another thread may call on a running
  node." The worker calls nothing on the node.

The hand-over is one bounded FIFO (`CloudHandover`, `handover_depth` clouds), one mutex
and one condition variable. The run() thread never waits on it: when it is full, the
oldest cloud is dropped and counted. At SIGINT or SIGTERM `run()` returns and the
application's destructor stops the queue; the worker processes what is queued and returns,
and the destructor joins it before the endpoints and the Node are destroyed.

## Diagnostics

The timer prints four lines every `report_period_ms` (500 ms), and the destructor prints them
once more at exit with `final` in place of `report`:

```text
report t=<s> imu received=<n> missing=<n> sequence_gap=<n> invalid_frame=<n> corrupt_payload=<n> schema_mismatch=<n> invalid_timestamp=<n> loan_budget=<n> loan_held=<n> loan_refused=<n>
report t=<s> cloud received=<n> missing=<n> ... (the same fields)
report t=<s> node timer_missed=<n> handover_dropped=<n> processed=<n> published=<n> publish_failed=<n> bad_cloud=<n>
report t=<s> tf static_base_link_lidar=<found|error> lookup_failed invalid_frame=<n> unknown_frame=<n> not_connected=<n> before_buffer=<n> after_buffer=<n> refused=<n>
```

- `t` is framework time, `hypha::now()`: host UTC live, the replayed `/clock` under
  simulated time.
- `received` counts the callbacks. `missing` is `missingSequences()`: frames lost before
  the subscriber read them, so a queue overflow. The other fields are
  `hypha::SubscriberMetrics` ([cpp-api.md §7](../../docs/cpp-api.md#7-service-limits-and-endpoint-queues)).
- `timer_missed` is `Timer::missedCount()`: the periods the timer never fired in.
- `handover_dropped` counts the clouds the hand-over queue dropped.
- `lookup_failed` counts the worker's lookups by `hypha::TransformError`. An
  `AfterBuffer` answer is asked again every 1 ms for at most `tf_wait_ms` (50 ms) of host
  time before it counts, because the /tf message at the cloud's stamp may still be in the
  run() thread's queue.
- `static_base_link_lidar` is a `lookupLatest()` of the static transform each report.

**Normal mode** (`processor.toml`). Every count except `received`, `processed` and
`published` stays at 0, and `published` equals the clouds received. In a run of 5.5 s:
`imu received=2218 missing=0`, `cloud received=56 missing=0`, `timer_missed=0
handover_dropped=0 processed=56 published=56`. The processor started 1 s after the
source still reports `static_base_link_lidar=found`: `/tf_static` is latched, and the
source sends both static transforms in one message (`StaticTransformBroadcaster`).

**Slow mode** (`processor_slow.toml`, `[slow] enabled = true`). The worker waits
`worker_delay_ms` (300 ms) per cloud, three cloud periods, and the timer callback blocks
the run() thread for `stall_ms` (1500 ms). `imu missing`, `cloud missing`,
`timer_missed` and `handover_dropped` grow; `published` stays below `cloud received`.
The framework also warns about the stall, `hypha: attachment 6 callback took 1500 ms,
threshold 10 ms`. A run of 5 s live:

```text
final t=1795038610.108 imu received=603 missing=801 sequence_gap=0 ...
final t=1795038610.108 cloud received=9 missing=26 sequence_gap=0 ...
final t=1795038610.108 node timer_missed=4 handover_dropped=2 processed=7 published=7 publish_failed=0 bad_cloud=0
```

The IMU queue holds 200 samples, 0.5 s at 400 Hz, so a stall of 1.5 s loses samples.

## Policies and start order

`sensor_pipeline_qos.h` declares `imu` (`maxDepth = 200`, the `queue_size` of the ROS
node) and `lidar_cloud` (`maxDepth = 4`, `maxPayloadDynamic = 270000` bytes). Every
typed endpoint of these types declares them through `hypha::QosOf<M>`, so the source and
the processor agree whichever starts first. `/tf` and `/tf_static` carry hypha's own
`tf` policy.

A player or a bridge declares no policy: when it opens a topic first, it creates the
topic with `default` (depth 32), and a typed endpoint with another policy is then refused
with `QosMismatch` ([cpp-api.md §7](../../docs/cpp-api.md#7-service-limits-and-endpoint-queues)).
Try the wrong order:

```sh
HYPHA_DOMAIN=demo target/release/hypha play --input examples/sensor_pipeline/fixture/sensor_inputs.mcap --loop &
HYPHA_DOMAIN=demo build/examples/sensor_pipeline/hypha_cloud_processor examples/sensor_pipeline/config/processor.toml
```

The processor exits with code 78:

```text
[ERROR] [hypha/transport] hypha_iceoryx2: topic '/imu': the declared QoS policy 'imu' (max_publishers 2, max_subscribers 4, max_depth 200) differs from the policy 'default' (max_publishers 8, max_subscribers 16, max_depth 32) its service was created with; this endpoint is refused
[ERROR] [cloud_processor] hypha: ports.inputs.imu failed: QosMismatch
```

Stop the player (`kill %1`). The order that works: start the processor first; it creates
`/imu` and `/lidar/points` with its policies and `/tf`, `/tf_static` with `tf`. The player
then opens the existing topics and publishes into them; `replay_check.sh` does this.

## The replay recipe

`replay_check.sh` replays `fixture/sensor_inputs.mcap` into the processor three times and
records the processor's output each time. It needs nothing installed:

```sh
./build.sh --tests
examples/sensor_pipeline/replay_check.sh          # or: ctest --test-dir build -R sensor_pipeline
```

One replay, as the script runs it:

1. The processor first, on simulated time:
   `HYPHA_USE_SIM_TIME=1 hypha_cloud_processor config/processor.toml`. The script waits
   for its line `cloud_processor: ready`. It then owns the topics' policies.
2. The recorder, on the output only:
   `hypha record --sync --topics /cloud_processor/centroid --compression none -o run_a.mcap`.
   The script waits for its line `Following`. With `--sync` the recorder takes part in
   the replay: it holds each chunk until it is written
   ([tools.md](../../docs/tools.md#hypha-record--capture-to-mcap)).
3. The player, in the foreground, with the input topics only:
   `hypha play --input fixture/sensor_inputs.mcap --clock --rate 1 --settle 0.5 --nodes hypha_record --topics /imu /lidar/points /tf /tf_static`.
   `--clock` publishes `/clock` from the recording's time, and every replayed frame
   carries its recorded time; the payload, and so each header stamp, is replayed
   unchanged. `--nodes hypha_record` makes the start wait for the recorder, and the
   player waits for it at every chunk. The processor is a plain callback node and is
   not waited for (#153).
4. SIGTERM to the processor, which prints its `final` lines, then SIGTERM to the
   recorder, which finishes its file.

The script then checks:

- each normal run accounts for every cloud it received: processed or dropped at the
  hand-over, every processed cloud published, none failed or bad, and the static
  transform found;
- the two normal runs match: when neither lost a cloud, every centroid pairs by stamp
  (`sensor_pipeline_compare compare run_a.mcap run_b.mcap /cloud_processor/centroid
  1e-9`, the fixture's 39 on a quiet host); otherwise every centroid both runs published
  is the same (`sensor_pipeline_compare common`). The replay does not pace the
  processor, so a loaded host can make the hand-over drop a cloud.

It stops only the processes it started, uses its own domain `sp_<pid>`, and at the end
removes only that domain's files in `/dev/shm` and `/tmp/iceoryx2`, after `lsof` shows
that no process holds them. One run takes 17 s; ctest gives it 120 s.

The output of one run, shortened:

```text
replay_check: run_a: hypha play --clock --rate 1 --nodes hypha_record
hypha play: effective rate 1.00x: 4.500 s of data in 4.501 s; set by --rate 1.00x
final t=1795038799.029 imu received=796 missing=0 sequence_gap=0 ...
final t=1795038799.029 cloud received=39 missing=0 sequence_gap=0 ...
final t=1795038799.029 node timer_missed=0 handover_dropped=0 processed=39 published=39 publish_failed=0 bad_cloud=0
final t=1795038799.029 tf static_base_link_lidar=found lookup_failed invalid_frame=0 unknown_frame=0 not_connected=0 before_buffer=0 after_buffer=0 refused=0
compare: 39 messages of /cloud_processor/centroid match by stamp; largest difference 0 m, tolerance 1e-09 m
replay_check: passed
```

What the lines show:

- **Source stamps.** `t` is the replayed time, a time of the recording, and the compared
  stamps are those of the recorded clouds. A centroid carries its cloud's
  `header.stamp`, never the host time.
- **Retained transforms.** The processor started after the fixture was recorded and
  still found `base_link` to `lidar`: the player replays the recorded `/tf_static` message
  and publishes that topic latched. Live, a processor started after the source finds it the
  same way. The first report of a replay is written at the first `/clock`, before any
  data, and shows `static_base_link_lidar=UnknownFrame`.
- **Loss.** None on a quiet host at `--rate 1`: the processor keeps up. Nothing waits
  for it, so a stalled processor loses samples as it does live.
- **QoS mismatch.** The wrong start order of the previous section, with the fixture.

### What the replay paces

Only the recorder. The processor follows `/clock` with plain callbacks, which take no
part in the replay lockstep (#153): they run as their samples arrive, and the player
does not wait for the processor
([execution-model.md §11.2](../../docs/execution-model.md#112-plain-callbacks-in-a-replay)).
Its two runs match when it keeps up, which `--rate 1` leaves room for; a node that
needs a deterministic replay at any rate is built from cycles. A centroid carries its
cloud's stamp, so the comparison by stamp holds.

### Compare against a ROS golden

The comparison of two hypha runs shows that the node is deterministic on one fixture.
To compare with the ROS node:

1. Record the ROS node's inputs and its output in one bag, and convert it:
   `mcap convert golden.bag golden.mcap` (Step 6 of the migration guide).
2. Use the bag's inputs as the fixture: `replay_check.sh --fixture golden.mcap` replays
   only the input topics, so the golden output never reaches the live output topic.
3. Compare one hypha output with the golden, by stamp. When the ROS node published under
   another name, give it last:
   `sensor_pipeline_compare compare golden.mcap run_a.mcap /cloud_processor/centroid 1e-6 /locator/centroid`.

The program pairs the messages in stamp order and fails on the first stamp, frame or
coordinate that differs beyond the tolerance, and on a different count, printing the
stamp:

```text
compare: message 0: the stamps differ
compare: expected stamp 1795038795.035044277
compare: actual   stamp 1795038797.335266161
```

Exit codes: 0 match, 1 differ, 2 a file or topic that cannot be read. Choose the
tolerance from the ROS node's own arithmetic; two hypha runs differ by 0 m.

## Re-record the fixture

From the repository root, after `./build.sh`:

```sh
export HYPHA_DOMAIN=fixture
target/release/hypha record --topics /imu /lidar/points /tf /tf_static --compression zstd \
    -o examples/sensor_pipeline/fixture/sensor_inputs.mcap &
# when the recorder prints "Following":
build/examples/sensor_pipeline/hypha_sensor_source examples/sensor_pipeline/config/source_fixture.toml
# the source stops after 4.4 s (880 ticks, 44 clouds); then stop the recorder:
kill -INT %1
```

The recorder follows a topic once its publisher exists, so it misses the first samples:
the committed file holds 1 `/tf_static`, 796 `/imu`, 397 `/tf` and 39 `/lidar/points`
messages, 4.5 s of data. `replay_check.sh` reads the cloud count from the file, so a new
recording needs no edit of the script.
