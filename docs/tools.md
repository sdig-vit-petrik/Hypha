# CLI Tools

Twelve tools ship in a single binary: `hypha <tool>` (aliased as `hy <tool>` and symlinks `hy<tool>`, e.g. `hyplay`, `hytop`). Run any tool with `--help` for exact flags. `hypha_ros_bridge` and `hypha_msggen` remain standalone binaries.

- **Node naming:** Observer tools (`top`, `topic`, `node`, `record` without `--sync`) join as `_hyphacli_<tool>` (multiple instances allowed, hidden from `hypha top`, dead instances auto-reclaimed). Publishing/active tools (`play`, `record --sync`, `intercept`, `foxglove`) take a unique node name per domain. `convert`, `param`, and `clean` do not join the bus.
- **Stream following:** `topic echo/hz/bw`, `record`, `top`, `foxglove`, and `hypha_ros_bridge` follow new `(topic, MD5)` streams via `StreamFollower` (graph-event driven, rate-limited helper thread, 3 observer slots per stream). Late-joining publishers report any frames missed before the tool subscribed (`--sync` fails on them).
- **Schema resolution:** All tools decode frames via `SchemaIdentityCache` from retained publisher schema records (max 64 nesting levels, 65,536 expanded messages).

---

## Observing the Bus

### `hypha topic`

```text
hypha topic list [-v]
hypha topic echo <topic> [-p] [--count <n>] [--type <type>] [--md5 <prefix>]
hypha topic hz   <topic> [--window <n>] [--type <type>] [--md5 <prefix>]
hypha topic bw   <topic> [--type <type>] [--md5 <prefix>]
```

- `list`: snapshots all active streams once (`-v` adds publisher/subscriber counts). Topics with multiple schemas print one line per `(type, md5-prefix)`.
- `echo`, `hz`, `bw`: follow all streams of `<topic>` (or filter with `--type` / `--md5`). `echo -p` outputs CSV (`%time` in framework send ns + flattened fields) and requires a single stream. `hz` measures from publisher send stamps, not reader arrival time.

### `hypha top`

```text
hypha top [--intercept-dir <dir>]
```

Interactive TUI monitor (`_hyphacli_top`, single thread, Mycelium truecolor palette with non-color status symbols):
- **Views (`1`–`5`):** Topics (per-stream rate/bw/type/MD5), Nodes (CPU %, RSS, cycle counters, `STAGNANT`/`IGNORED`/`DROP/REJ/UNDEC`, `OTHER SCHEMAS` mismatch alerts), Intercept, Inspector, and Graph.
- **Global keys:** `1`–`5` tabs, `j/k`/arrows move, `PgUp`/`PgDn`/`Home`/`End`, `Enter` inspect, `/` filter, `s` sort, `Space` pause, `q` quit.
- **Intercept tab (`3`):** drives `hypha intercept` by editing its TOML config (restricted to `--intercept-dir`, default cwd). Keys: `Tab`/`Shift-Tab` switch pane, `Enter` edit/apply, `Esc` cancel/back, `Space` toggle bit/bool, `x`/`X` clear override(s), `a` arm/bypass, `n` new interception, `d`/`Del` delete.
- **Graph tab (`5`):** draws publisher→subscriber graph from `~/introspection` manifests (`●` ok, `▲` waiting, `✖` degraded, `◌` silent, `○` dead, `∅` no manifest) and flags feedback vs. unbounded triggered loops. Keys: `g` toggle topic nodes, `i` toggle internal topics, `e` export `hypha_graph.dot`.

### `hypha node`

```text
hypha node list
hypha node info <name>
```

Lists alive and dead nodes in `HYPHA_DOMAIN` from the transport registry (`info` shows domain and `~/state` topic).

---

## Recording, Replay, and Conversion

### `hypha record`

```text
hypha record [--topics <pattern>...] [--output <file.mcap>]
             [--compression <lz4|zstd|none>] [--sync] [--nodes <name>...]
```

- Records matching streams (`*` one segment, `**` suffix; default `**`) into self-describing MCAP (default `lz4`). Multi-schema topics write one MCAP channel per stream.
- `log_time` and `publish_time` store the frame's framework send stamp (ns since epoch).
- **Observer mode (default):** subscribes at service max depth; reports queue loss every 10 s and at exit, exiting **1** if any sample was lost.
- **Lockstep mode (`--sync`):** joins as `hypha_record` and holds each `hypha play --clock` chunk until written; fails immediately on any loss or late-joining mid-chunk stream. Use `--nodes` to wait for specific participants before the first chunk ([replay-flow-control.md](replay-flow-control.md)).

### `hypha play`

```text
hypha play [--input <file.mcap>] [--rate <factor|max>] [--loop] [--clock]
           [--nodes <name>...] [--settle <s>] [--chunk <ms>] [--timeout <s>]
           [--topics <t>...] [--start-utc <ts>] [--end-utc <ts>]
```

- Replays messages in recorded timestamp order (one publisher per `(topic, MD5)` stream; never drops messages to catch up; never replay an actively written file). Start nodes first if using non-default QoS policies.
- **`--clock` (lockstep, default):** publishes `/clock` in `--chunk` ms windows (default 100) and waits on `/clock_progress` for all participants (plus `--nodes`). Refuses to start if `/clock` already has a publisher. `--loop` / restart sends `/clock` backward, triggering participant exit **75** and supervisor restart.
- **Open-loop mode:** set `HYPHA_REPLAY_LOCKSTEP=0` on player and nodes to disable `/clock_progress` barriers (disallows `--rate max`, `--nodes`, `--settle`, `--timeout`).
- **Windowing:** `--start-utc` (inclusive) and `--end-utc` (exclusive) require ISO-8601 timestamps with `Z` or offset.
- **Interactive keys (foreground TTY):** `Space` pause, `s` step chunk, `+`/`-` speed, `r` restart, `q` quit.

### `hypha convert`

```text
hypha convert <run.db3> -o <run.mcap> [--msg-path <dir>...] [--topic <t>...] [--xyzirc <topic>]
```

Converts a ROS 2 SQLite bag (`.db3`; decompress `.db3.zstd` first) from CDR to ROS 1 MCAP using embedded or `--msg-path` definitions. `--xyzirc <topic>` converts 16-byte XYZI `PointCloud2` to Autoware 2.1.0 XYZIRC.

---

## Running, Inspecting, and Cleaning Systems

### `hypha launch`

```text
hypha launch up|attach|down <file.toml> [--session <name>] [--attach]
```

Runs each `[[units]]` entry in its own window of a detached `tmux` session (requires `tmux ≥ 3.0`):

```toml
[session]
historyLimit = 10000          # 100..1000000

[[units]]
name = "planner"              # reserved: hypha_launch_setup
exec = "/opt/hypha/bin/planner"
args = ["--rate", "50"]
workingDirectory = "/var/lib/hypha"
restart = "on-failure"        # no (default) | on-failure | always
restartDelaySeconds = 1.0     # doubles up to 30 s; resets after 60 s uptime; gives up after 10 failures
after = ["lidar"]             # start order only (no readiness wait)

[units.env]
HYPHA_DOMAIN = "robot1"
```

- **Exit-code policy:** `78` (config error) never restarts; `75` (clock rewound) always restarts immediately with `HYPHA_REJOIN_AFTER_NANOS` (not counted against restart cap); `0` restarts only under `always`; other codes/signals follow `restart`.
- **Shutdown:** `Ctrl-c` or `down` sends `SIGTERM` to the unit's process group, escalating to `SIGKILL` after 5 s (10 s overall for `down`). On Linux, parent-death `SIGTERM` and `pidfd` reaping prevent orphans.
- **Limits:** stopping the tmux server stops all units; high-rate stdout logging can block writers; units run in background process groups (no TTY input).

### `hypha param`

```text
hypha param dump|validate <file>
hypha param get <file> <key>
```

Offline TOML inspection using the exact node loader (`include`, `${ENV}`, conflict checks). Exit `0` valid/found, `1` invalid/absent.

### `hypha clean`

```text
hypha clean [--domain <prefix>]
```

Removes unused iceoryx2 shared-memory segments (`/dev/shm`, `/tmp/iceoryx2`) for `HYPHA_DOMAIN` (or all domains matching `<prefix>`). Never kills processes; skips domains with live nodes (or live test runner PIDs for `t_<pid>_<n>`). Exit codes: `0` clean, `2` kept in-use domain(s), `1` error. Use `./clean-shm.sh [--force]` in the repo to wipe all domains on the host.

---

## Fault Injection and Load Testing

### `hypha intercept`

```text
hypha intercept [--name <backend>] [--config <path>]   # defaults: default, ./intercept.toml
```

Headless man-in-the-middle relay (`hypha_intercept_<backend>`) that rewrites decoded fields on the fly while preserving producer send timestamps and schema identity. Reloads config on file change and publishes latched TOML status on `/hypha/intercept/<backend>/state`:

```toml
revision = 7

[[interception]]
name   = "battery"
input  = "/battery/raw"
output = "/battery"
armed  = true                 # false = pass through unchanged

[interception.overrides]
"cells[2].voltage" = { value = "0.0" }
"status"           = { set = "0x0002", clear = "0x0008" }
"serial_number"    = { value = "ZZZ\\x00" }
```

Byte arrays cannot be overridden and array lengths cannot change. Schema changes halt forwarding (`state = "schema_changed"`) until re-armed.

### `hypha load`

```text
hypha load <workload.toml> --report <out.json> [--recording <out.mcap>]
```

Spawns multi-process Rust SDK workers (`[[process]]`, `[[topic]]`, optional `[recorder]`) in an isolated `load_<pid>` domain, measures end-to-end `CLOCK_MONOTONIC` latency (log-linear histogram, ≤1.6% error), sequence loss, missed periods, RSS, and Linux `/dev/shm` growth, and checks `[limits]`:
- **Exit codes:** `0` all limits passed, `1` limit failed, `70` run/workload/worker failure.
- **Limits:** `latency_p99_us`, `max_interval_latency_p99_us`, `max_loss`, `loss_accounted`, `max_refused_publishes`, `max_publish_call_us`, `max_missed_periods`, `max_unrecorded`, `max_rss_growth_bytes`, `max_shm_growth_bytes` (Linux only; `null` on macOS).
- **Standard workloads:** `scripts/rated-load.sh` (`examples/load/rated.toml`, NFR-03/04: 46 topics, 72.5 MB/s, 9,960 msg/s) and `scripts/overload.sh overload|isolation-latency` (`examples/load/overload.toml`, NFR-05).

---

## Bridging

- **`hypha_ros_bridge`:** Bridges ROS 1 Noetic (XML-RPC/TCPROS) and Hypha topics as opaque bytes; transport age starts at the bridge. See [apps/hypha_ros_bridge/README.md](../apps/hypha_ros_bridge/README.md).
- **`hypha foxglove`:** WebSocket server for Foxglove Studio. Display-oriented (queue depth 8, drops frames older than `--max-latency` default 500 ms, throttles `/clock` to 50 Hz, optional `--max-rate <Hz>` and `--topics` filter); caches latest per channel and merges `/tf_static` by `(parent, child)` frame pair.
