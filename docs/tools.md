# Command-line tools

hypha ships thirteen tools in one executable, `hypha <tool>`: `topic`, `top`, `node`, `service`,
`record`, `play`, `convert`, `intercept`, `foxglove`, `launch`, `param`, `clean` and `load`.
`hypha --help` lists them, and `--help` on a tool gives its exact options: the binary is the
authority on its flags, and this page is a map.

- **Short names.** `hy` is `hypha` (`hy play --input run.mcap`), and each tool has a link
  `hy<tool>`: `hytopic`, `hytop`, `hynode`, `hyservice`, `hyrecord`, `hyplay`, `hyconvert`,
  `hyintercept`, `hyfoxglove`, `hylaunch`, `hyparam`, `hyclean`, `hyload`. The install makes them;
  the old names `hypha_<tool>` are gone. Messages say `hypha play: ...` whatever name started the
  tool. In CMake, `hypha::<tool>` (`hypha::topic`, ...) runs a tool's link. `hypha_ros_bridge` and
  `hypha_msggen` are separate executables.
- **Bus nodes.** The tools join the bus through the framework, never around it. `hypha convert` and
  `hypha param` work on files and do not join; `hypha clean` removes a domain's resources without
  joining.
- **Names.** `hypha top`, `topic`, `node`, `service`, and `record` without `--sync` join as CLI tool
  nodes, `_hyphacli_<tool>`: any number may run in one domain, `hypha top` does not list them, and
  the next node that starts reclaims a dead one. `hypha play`, `hypha record --sync`,
  `hypha intercept` and `hypha foxglove` use a name of their own, so a second one with the same name
  in a domain is refused.
- **Build.** `./build.sh` puts `hypha` in `target/release/`; the build tree links `hy` and the
  `hy<tool>` names to it in `build/apps/tool-links/`.
- **Colours.** `hypha top`, the `hypha launch` tmux session and the `hypha play` progress line share
  one palette, Mycelium (`apps/hypha_tools/src/theme.rs`). It needs a truecolor terminal and no
  special font. State shows in form as well as colour (`✗`, `[PAUSED]`), so each view reads without
  colour.

## Observing the bus

### `hypha topic` — what is being published

```text
hypha topic list [-v]                     every topic of the bus, one line per type; -v: publishers and subscribers
hypha topic echo <topic> [-p] [--count <n>] [--type <type>] [--md5 <prefix>]
                                          print each message, decoded
hypha topic hz <topic> [--window <n>] [--type <type>] [--md5 <prefix>]
                                          publish rate of each stream
hypha topic delay <input> <output> [--window <n>]
                                          time from an input topic to an output topic
hypha topic bw <topic> [--type <type>] [--md5 <prefix>]
                                          bandwidth and mean message size of each stream
hypha topic pool --payload <bytes> [--publishers <n>] [--subscribers <n>] [--depth <n>] [--loans <n>]
                                          what a publisher's pool reserves
```

**`pool`** joins no bus. It prints the slots, slot size and shared memory of a publisher whose
messages are at most `--payload` serialized bytes under a policy of these limits (an omitted limit is
the `default` policy's: 8, 16, 32), from the rule the transport sizes with. `hypha topic pool
--payload 5038896 --publishers 1 --subscribers 2 --depth 2` is a 5 MP mono8 camera on
`hypha::imageQos`: 23 samples of 5039000 bytes, 115897000 bytes (cpp-api.md §7.1).

**`list`** reads the transport's service listing once and returns; it listens to nothing.

- A topic is listed once a publisher, or a subscriber that declared its policy, opened it, sent or
  not, so a node with silent topics shows them.
- A line names the topic and type, `/odom  [nav_msgs/Odometry]`. A topic with two types, or two
  versions of one, is two lines, each with the first eight hex digits of its MD5: `/odom
  [nav_msgs/Odometry (cd5e73d1)]` (cpp-api.md §5). A stream whose publisher gave no definition shows
  its schema hash, `[? (0123456789abcdef)]`.
- `-v` adds the endpoints the listing counts for free: `/odom  [nav_msgs/Odometry]  1 publisher, 2
  subscribers`. A crashed publisher counts until its node is reclaimed.
- An empty bus prints nothing on standard output and `no topics on the bus` on standard error.
- The listing opens each stream for an instant, taking one node slot of it meanwhile
  (interface-specification.md), and reads every service of the domain, so a bus of many streams
  takes longer.

`hz` and `bw` cannot measure a topic that is not publishing now.

**A topic of several streams.** A topic carries one stream per schema, and `echo`, `hz` and `bw` read
all of them, each on its own. A stream that joins after the tool started (a second version during a
migration) is followed once the tool hears the graph event of its publisher or subscriber ("Following
streams" below), and the tool says on standard error how many samples it had already sent. A tool
started before any publisher waits, says so once, and follows the first stream that appears. With one
stream the output is plain; with more, each stream says which it is:

- `echo` prints a `# <type> (<md5 start>)` comment after each message's `---`. `echo -p` prints one
  table, so with several streams it prints nothing, names them with type and MD5 start, says to pick
  one with `--type` or `--md5`, and exits 1.
- `hz` and `bw` keep one window per stream and print a title line, `<topic> [<type> (<md5 start>)]:`,
  before each stream's figures.

`--type <type>` follows only streams of that type; `--md5 <prefix>` only streams whose MD5 starts with
those hex digits (case-insensitive); together, both. A filter needs the stream's schema: a stream
whose publisher gave no definition matches none until a publisher with one joins. When no stream
matches, the tool lists the name's streams (one without a definition by its hash) and waits. One
selected stream prints as a single-stream topic. The flags are the same on all three subcommands.

**`echo -p`** prints one CSV row per message, as `rostopic echo -p` does.

- The first line is the header: `%time` and the column names, taken from the first message and not
  printed again.
- `%time` is the frame's framework send stamp in nanoseconds: the time its publisher gave it, UTC on
  host time. For a cycle output that is the step's time: on host time the wall time at the step's
  start (so a triggered step's output is stamped later than its trigger sample, and does not carry
  the trigger's stamp); on simulated time the step's time, which for a triggered step is the trigger
  stamp T. For a replayed message it is the recorded time.
- A nested field is `outer.inner`, an array element `field0`, `field1`, a `time` `field.secs` and
  `field.nsecs`. A `bool` is `1` or `0`. A float keeps its full precision.
- A string is written as is, unquoted, so a colour escape code reaches the terminal, and a comma or
  line break breaks the row.
- A message that cannot print (schema not resolved, payload does not match) is reported on stderr and
  gives no row. `echo` refuses an unknown option or a `--count` without a number.

To publish a debug string only while someone reads it, check the publisher's `subscriberCount()` first
(C++, `docs/cpp-api.md` section 4).

**Decoding.** `echo` resolves the frame's compact hash against the publisher's retained schema record
and prints named fields. The record can reach a new reader a moment after the first frame, so `echo`
waits briefly and retries. Without a schema it says so instead of guessing, and a payload that does not
match its schema ends with a line that says so. The decoders follow at most 64 levels of nested
messages (`MAX_NESTING_DEPTH`, the limit `hypha_msgdef` also applies): a recursive or deeper
definition ends the same way instead of aborting, and so does one that expands to more than 65 536
nested messages (`MAX_EXPANDED_MESSAGES`), before any message is decoded.

Every tool that names a message type resolves the schema hash the same way
(`hypha::SchemaIdentityCache`): `hypha topic`, `hypha top`, `hypha record`, `hypha foxglove` and the ROS
bridge. There is no compiled-in fallback; a frame carries only the hash, so a tool that skips this step
cannot report a type name, MD5 or definition.

**`hz`** measures the cadence from the stamp each publisher wrote into the frame, not from when this
process read it, so a slow reader does not distort the rate.

**`delay`: how long a node takes from an input to its output.** `hypha topic delay <input> <output>` is
the `rostopic delay` of a node that has no header to compare. It follows both topics and, for each
output frame, takes the time since the newest input frame sent at or before it. Both times are the host
monotonic send stamps in the frame headers, so it needs nothing from the node, decodes no payload (a
1.6 MB cloud costs what an `Int32` costs) and does not depend on the reader's speed. Once a second it
prints the last `--window` outputs (default 100):

```text
$ hypha topic delay /points /points_filtered
delay: median 1.412 ms, p99 2.903 ms, max 3.104 ms (min 0.912 ms) over 100 outputs
```

- For a triggered node it is the time from the trigger sample to the output, including the step: a
  cycle output takes its transport stamp when the step sends it. For a rate node it is the newest
  input's age when the output leaves, so it includes the wait for the next step (half a period at the
  median).
- A node with several inputs has one delay per input: name the input you want.
- The tool keeps the input stamps of the last 5 s (at most 65 536). An output sent before all of them
  has no delay and is counted in the line: `, 3 outputs had no input before them`.
- Every stream of a topic name counts, and `<input>` and `<output>` must differ. Both topics must be
  sent on this host: two hosts' stamps are not one clock. A replay measures the real time the node
  takes, not the recorded one.
- A node's step times alone, without a second topic, are in `~/state`; `hypha top` shows them.

### `hypha top` — live bus monitor

```
hypha top [--intercept-dir <dir>]
```

`--intercept-dir <dir>` names the directory the Intercept tab may read and write config files under
(default: the working directory at start). A config path the backend names on the bus is used only if,
with `..` and symbolic links resolved, it lies inside that directory. Any other path, relative ones
included, is refused: the tab neither reads nor writes it, and the status bar shows `refused: <path> is
outside <dir>`.

An interactive terminal view of topic rates and bandwidth, a node registry, and a message inspector.
It resolves frame hashes against each publisher's schema record before showing type and fields.

- **Nodes.** A `/<node>/state` or `/<node>/introspection` topic makes a node entry only if its frames
  carry that `hypha_msgs` type's hash; a same-named topic of another type is ignored. A recording made
  before `hypha_msgs/NodeState` gained its cycle and counter fields carries the old hash, so its replay
  shows nodes without a state. CLI tool nodes (`_hyphacli_*`), this monitor included, are not listed.
- **Node details.** One line per cycle: steps, missed periods, step time in ms (median, 99th percentile
  and maximum since start and over the last 10 to 20 s), the last step, overruns, output drops. Then the
  log counters, and per input the words `STAGNANT` and `IGNORED` and the `DROP/REJ/UNDEC` counts.
- **Application checks** (`hypha::DiagnosticUpdater`, `NodeState.checks`) show under the health line:
  one `Check: <name>: <LEVEL> <message>` line per check that is not `OK`, `ERROR` and `STALE` first in
  red, `WARN` in yellow, and `... and n more` past four. All `OK` is one line, `Check: 3 OK`; a node that
  reports none shows no line. A node degraded by a check says so in its badge (`DEGRADED / FAILED
  CHECK`, or `STALE INPUTS, FAILED CHECK` when an input is also not usable), and the node list and Graph
  tab show it `DEGR` and `✖`. A `WARN` check changes neither.
- **Streams.** A topic of several streams is one row per stream, each with its type and MD5 start
  (`nav_msgs/Odometry (cd5e73d1)`); a stream that joins later is followed through the graph event. An
  input binding shows its type and MD5, is judged by its own stream, and shows in `OTHER SCHEMAS` what
  its node found of the topic's other versions: `not checked` before the node's first check (and for a
  node whose default `WaitSet` does not run), `none`, or each live version by type and MD5 start. A
  version that is published while the input has no publisher of its own is red, and the node list's
  `INPUTS` column adds `1 OTHER VERSION` to a waiting or stale count.
- **Keys.** `1` to `5` select a view, `j`/`k` or the arrows move, `PgUp`/`PgDn` move a page, `Home`/`End`
  jump to the ends, `Enter` opens the inspector, `/` searches, `s` changes the sort (name, then
  bandwidth, rate, messages), `Space` pauses, `q` quits. A long list scrolls to keep the selection
  visible.
- **Exit.** 0 after a normal quit. When its loop fails (for example, the node registry cannot be
  listed) it restores the terminal, prints `hypha top: <error>` on standard error and exits 1.
- **CPU and memory.** The node list shows each process's CPU over the last second (100 % is one core, as
  in `htop`, to a tenth of a percent) and its resident memory. On Linux it reads the process CPU clock
  and `/proc`; elsewhere `ps`, whose CPU time counts in 10 ms steps. A process it cannot read shows `-`.
- **Cost.** The monitor is one node (`_hyphacli_top`) and starts no thread. Topics and traffic appear at
  once; schema identities fill in later, one lookup per render turn, which reads the record once and
  never waits. A missing record is asked for again after 100 ms, and the topic shows no type meanwhile.

**The Intercept tab** (`3`) drives a running
[`hypha intercept`](#hypha-intercept--rewrite-fields-between-a-producer-and-its-consumers). It finds the
backend by its state topic, edits the backend's config file, and shows each field's In and Out values
from traffic it already receives; it needs no type of the intercepted topic.

- **Layout.** The left column holds the interceptions and the selected one's details (input and output
  with rates, message type, armed or bypass, relay state, frame counters, newest error). The right
  column holds the selected input's field tree above the override editor: dec/hex/bin and a tri-state
  bit grid for an integer, the value and its bit pattern for a float, a toggle for a bool, `\xNN`
  escapes for a string, `sec.nsec` for time. The tree's `ovr` column shows `V` for a value override,
  `B` for bits, `S` for an override below that row, and `–` for a path the output lacks.
- Config files stay inside the intercept directory (above).
- **Status bar.** `● ARMED` or `○ BYPASS`, the newest error, `◌ pending` until the backend applied the
  written revision, `⚠ backend offline` when no state arrived for 2 s. The footer lists the focused
  pane's keys.
- **Width.** From 110 columns both columns show; from 80 a one-row selector (with the input's type)
  replaces the left column; from 60 one pane shows at a time with a breadcrumb; below 60×12 the tab
  shows only the size it needs. A resize keeps the selection and any text being typed.
- **New.** `n` opens the new-interception modal. Type input, output and name in that order; output
  (`<input>/intercepted`) and name (`/battery/raw` → `battery_raw`) follow the input until typed. Under
  a topic field the matching bus topics are listed with their type, prefix matches first: `↑↓`
  highlights one, `Enter` or `Tab` takes it. The input's type and rate show under it. `Enter` with no
  highlight adds the interception; `Esc` drops a highlight, then the modal.
- **Delete.** `d` (or `Del`) asks in a modal: `y` deletes the interception with its overrides, any other
  key keeps it.

| Key | Interceptions | Fields | Override |
|---|---|---|---|
| `↑↓` `j k` | select | move | – |
| `←→` `h l` | – | collapse / expand (`h` on a leaf: parent) | move the cursor, or the bit of an integer |
| `Tab` / `Shift-Tab` | next / previous pane | next / previous pane | next / previous pane |
| `Enter` | to the fields | edit the field | apply the value |
| `Esc` | – | back to the list | cancel the edit |
| `Space` | (pause) | (pause) | bit: keep → set → clear; bool: toggle |
| `x` / `X` | – | clear the field / all overrides | (types `x`) |
| `Ctrl-x` | – | – | clear the field |
| `a` | arm / bypass | arm / bypass | (types `a`) |
| `n` | new (modal) | – | – |
| `d` / `Del` | delete (`y` confirms) | delete (`y` confirms) | (types `d`) |

In the Override pane every printable key is text, so `q` and the digits do not switch tabs; `Esc` leaves
the pane.

**The Graph tab** (`5`) draws who publishes to whom, left to right as `rqt_graph` does, from the latched
`/<node>/introspection` manifests.

- **Health.** `●` ok, `▲` waiting for an input, `✖` degraded, `◌` silent (no `~/state` for 3 s), `○`
  dead, `?` no state. `∅` marks a node without a manifest (Rust and Python nodes, tools such as `hypha
  play`), drawn without edges. CLI tool nodes are not drawn.
- **Keys.** `g` switches between nodes only (one edge per node pair) and nodes with topics (rounded
  boxes; a topic with no publisher or no subscriber is grey). `i` shows `/clock`, `/hypha/...` and the
  `~/state` and `~/introspection` topics, hidden by default. `/` keeps the matching nodes and topics and
  their direct neighbours. `↑↓` select, `←→` and `PgUp`/`PgDn` pan, `Enter` opens the selection in its
  tab, `e` writes `hypha_graph.dot` in the working directory for Graphviz.
- A wildcard subscription is drawn to every bus topic it matches. The manifest is sent once, after
  setup, so an endpoint created later is not drawn.
- **Loops.** Nodes that reach each other through topics form a loop: two nodes that feed each other, a
  longer ring, or a node that reads its own output (`↻`). A loop is a warning, not an error: it may be
  intended, but it can surprise. Loops are listed under the graph in the warning colour, and the edge
  that closes one points back (`◀`). A *feedback* loop passes through a rate cycle, so it runs at a
  bounded rate, like a controller and its plant. A *triggered* loop is closed between triggered cycles
  and callbacks only, so one message can circulate without bound. A publisher that no cycle output owns
  counts as fed by every cycle and callback of its node, so a loop may be called triggered when a rate
  cycle in fact bounds it.

### `hypha node` — what is running

```text
hypha node list           every process the node registry knows about, alive or dead
hypha node info <name>    detail for each node of that name
```

It reads the transport's node registry within `HYPHA_DOMAIN`. The processes of one CLI tool share its
name, so `info _hyphacli_topic` can list several nodes. `info` shows the domain and the `/<node>/state`
topic for C++ cycle input health (watch it with `hypha topic echo`); Rust app nodes do not publish that
state. `info` lists no inputs, so it shows no stream or version: `hypha top` does. A detail the backend
cannot supply is printed as unavailable, never invented.

### `hypha service` — call a service

```text
hypha service list                               one line per service: name, type, MD5, server, clients
hypha service info <service>                     its type, server, clients, policy, call log and definitions
hypha service call <service> '<toml>' ...        call it and print the response
hypha service call <service>                     print the request's template
```

`call` is `rosservice call` with the request in TOML, the format of every hypha file. The keys are the
request's fields, and an omitted field takes its zero value. Several arguments are several lines of one
document (`'a = 1' 'b = 2'`); one argument may also hold several lines.

```text
$ hypha service call /camera/set_enabled 'data = true'
success = true
message = "streaming"
$ hypha service call /camera/set_exposure 'target_exposure = 20000'
reached_exposure = 10000.0
success = false
$ hypha service call /camera/set_enabled 'data = false'
hypha service: refused: not armed                      (exit 2)
$ hypha service call /camera/set_exposure
target_exposure = 0.0
$ hypha service call /move_to 'goal = { x = 1.0, y = 2.0 }' 'speed = 0.5'
$ hypha service call /move_to 'goal.x = 1.0' 'goal.y = 2.0'
```

| Field | TOML | Example |
|---|---|---|
| `bool` | a boolean | `data = true` |
| an integer type | an integer, range-checked (`_`, `0x` allowed); a `uint64` above 2^63 - 1 cannot be written | `count = 3` |
| `float32`, `float64` | a float or an integer; `nan`, `inf` | `gain = 0.5` |
| `string` | a quoted string, TOML escapes | `name = "left \"cam\""` |
| `time`, `duration` | `{ secs, nsecs }`, an omitted part 0 | `stamp = { secs = 7, nsecs = 500 }` |
| a nested message | an inline table or dotted keys | `goal = { x = 1.0 }`, `goal.x = 1.0` |
| an array, `T[N]` | an array; a fixed one of exactly N values | `ids = [1, 2]` |
| an array of messages | an array of inline tables, or `[[field]]` tables | `path = [{ x = 1.0 }, { x = 2.0 }]` |
| `uint8[]`, `byte[]`, `char[]` | an array of integers, or a string of its bytes | `blob = [104, 105]`, `blob = "hi"` |

- **Types are strict.** A TOML type other than the field's (`data = "true"`, `count = 1.5`) is refused,
  naming the field: `data: expected true or false, found the string "true"`. `rosservice`'s YAML and
  JSON are not read: `"data: true"` is refused with the TOML reason and its line.
- **Definitions.** The tool encodes with the definition the server registered when it started (a C++
  server registers both halves of its `.srv`), so it calls types it was never built with. A request of
  no fields (`std_srvs/Trigger`, `std_srvs/Empty`) needs no body, and a response of no fields prints `#
  answered: <type> has no fields`. A half of no fields is registered as an empty definition, so it is not
  a server that registered none.
- **Output.** The response and the template print as TOML in one form (nested messages as dotted keys,
  arrays and arrays of messages inline, every element), so either is a valid request of its type again.
  Anything else goes to standard error with its reason.
- **`--timeout <s>`** sets the wait (default: the service policy's, 1 s under `defaultService`): that
  long for a server to appear, then that long for the answer.
- **Slots.** The tool's client belongs to a CLI tool node, so it takes one of a service's three observer
  slots, never a declared client's; the server logs the call at INFO with the tool's name.
- **`info` and `list`** read the transport's listing of the domain. `list` names the server's node and
  pid (`dead` for one that crashed and is not reclaimed yet) and counts the clients other than the tool.

| Exit | Outcome |
|---|---|
| 0 | answered |
| 1 | the tool failed: an argument, a request that does not fit the type (the message names the field), a server that registered no definitions |
| 2 | refused, with the server's reason |
| 3 | no server: none appeared within the timeout |
| 4 | not sent |
| 5 | server lost: it crashed, left, or failed while it handled the call |
| 6 | timeout |

Not yet: per-service counts of answered, refused and timed-out calls (they come with the `~/state` rows
of stage 2), and a notice that a server on simulated time waits for a paused `/clock`.

**The call log.** Each client publishes a `hypha_msgs/ServiceCall` on `<service>/calls` as each call
ends: service, type and MD5, the client's node and port, the request's sequence, the outcome and reason,
the sent and done stamps, and the request and response bytes.

- A record is encoded only while the topic has a subscriber (one subscriber-count read per call). It
  never holds a call up: a record that finds no free sample is dropped, counted and logged, and readers
  see the gap in the frame sequence.
- It carries the first 16 KiB of request and response, whatever the policy allows, so the log costs no
  payload bound. `request_len` and `response_len` are the lengths before the cut; `truncated` is set
  when a half was cut (a cut half is not a decodable message).
- Size: at most 2 KiB plus the two halves as the policy bounds them or 16 KiB each (10 KiB under
  `defaultService`, 34 KiB under a policy of 4 MiB each way). A reader queues at most 4 records, and each
  client's log holds 26 in shared memory: 260 KiB under `defaultService`, 884 KiB under that 4 MiB policy.
- Read it with `hypha topic echo /camera/set_enabled/calls`. `hypha record` records it like any topic;
  `hypha play` never publishes it again, so a recording never calls a live server. The services
  themselves are not recorded.

### Following streams

`hypha topic echo`, `hz`, `bw`, `hypha record`, `hypha top`, `hypha foxglove` and the ROS 1 bridge's
automatic mode read topics whose schema they were not told, so each reads the streams that exist when it
subscribes. To follow a topic name through a migration they share one follower
(`hypha_node::sdk::StreamFollower`): it opens one subscription per stream (a topic name and a schema hash)
that matches its patterns and the `--type`/`--md5` filters, and one for each stream that joins later.

- **Learning of streams.** The follower hears the graph event a node's housekeeping hears: every
  publisher and subscriber that is created or dropped notifies it. The tool opens the event listener
  before its first listing, so no stream falls in between. A thread of the tool's own (the SDK creates
  none) sleeps until the event and then lists the streams: at most once per 100 ms, at most one fifth of
  its time (the next listing waits four times as long as the last took), and after 2 s with no event in
  case a notification was lost (the sender logs a failed one).
- **Subscribing.** The tool's loop takes what the thread found and subscribes to new streams by schema
  hash, with no listing of its own, so a stream without a definition is read too. Its type is looked up
  only for a `--type` or `--md5` filter and for a label. A `--sync` recorder lists on its own thread
  before it answers a chunk, because it must have every stream.
- **Cost** (release build, loaded machine at load average 15): a loop pass with nothing to learn takes 1
  µs (4 to 13 µs in debug); a listing, which reads every service of the domain, takes 70 ms with one
  stream and 120 ms with 100, on the helper thread; the first listing that follows 100 streams, with
  their subscriptions, 170 ms. A 1 kHz topic recorded for 6 s on a quiet graph lost 165 to 168 samples to
  the backstop listings when the loop listed itself, and 0 with the helper thread (5 runs of 8; 7, 14 and
  124 in the others, with a build running beside).
- **Slots.** Each followed stream takes one observer slot of its service per tool
  (`RESERVED_OBSERVER_SUBSCRIBERS`, 3). A fourth tool on one stream is refused: `all 3 observer slots of
  '<stream>' are taken by other tools`.

**What a late stream sent before it was followed.** A publisher's first frame has sequence 1, so the
first sequence a subscriber accepts from it, less one, is the number of its frames the tool never saw.

- The tools count them for publishers known to have come after them: every publisher of a stream that a
  later listing than the first showed, and publishers that joined a followed stream since a listing
  showed one more publisher on it. They do not count a publisher that ran when the tool started (its
  history is not the tool's to record) or a latched publisher's replayed sample (a latched stream is not
  judged). A second publisher joining a stream that had one is told from a quiet one by the listing's
  count alone.
- A publisher whose port registered after the tool subscribed owed it every frame, so what it sent before
  the tool's first accepted frame is a loss: reported as `lost`, failing the recording
  (`communication-integrity.md` §3).
- `hypha record` prints `hypha record: <stream>: <n> samples were sent before it was followed` as it
  finds them, and `<stream> sent <n> samples before it was followed` under `Recorded messages:`; the
  recording is not failed for it, since a node starting mid-recording cannot be avoided. `echo` and `hz`
  print one such line on standard error. `--sync` fails the recording on them.
- A stream is followed after the listing that shows it, and listings are spaced: at 100 Hz the recorder
  counted 17 to 73 samples before it, at 1 kHz 135 to 599, and the recording began at the counter it
  named (`record_late_streams.rs`, loaded machine; before the count existed the same late streams began
  at 22 to 55 and at 1397, with no word).

**Not followers.** `hypha topic list` lists the streams that exist when it runs, once. `hypha intercept`
relays one named input of one type, subscribing to the streams that exist at its start; a type change is
the configuration's. Neither follows a stream that joins later.

## Recording and replay

### `hypha record` — capture to MCAP

```text
hypha record [--topics <pattern>...]
           [--output <file.mcap>] [--compression <lz4|zstd|none>]
           [--sync] [--nodes <name>...]
```

It follows every topic that matches a `--topics` name or pattern (`*` is one segment, a trailing `**`
the rest; default `**`) and writes an MCAP file with chunk compression `lz4` (default), `zstd` or `none`.
Each topic's schema is recorded with the data, so a recording decodes later without the node that sent
it.

**Losses without `--sync`.** The recorder is an observer, and each topic is subscribed with depth 0,
which the transport reads as the deepest queue the topic's policy admits (32 under `default`). When a
topic overruns that queue, its oldest samples are lost; the subscriber counts the missed sequence numbers,
so a loss is not a sample the recorder dropped.

- Every 10 s and at the end it reads each topic's cumulative count, and when it grew prints `hypha record:
  <topic>: <n> samples lost: ...` on standard error, naming the remedy (a larger `maxDepth` in the topic's
  policy, or `--sync`).
- Under `Recorded messages:` it lists each such topic as `  <topic> lost <n> samples` and exits 1: the
  recording is incomplete.
- A sample that cannot be recorded is dropped with one warning per topic. That is not a loss and does not
  change the exit code.
- To record a fast topic completely, run `hypha record --sync` beside `hypha play --clock`.

**`--sync`** makes the recorder a participant of a `/clock` replay (`replay-flow-control.md` §6.3).

- It reports under the node name `hypha_record` (the name `hypha play --nodes` takes), so one runs per
  domain. It holds each chunk until it is written, so the recording holds every output of the replay.
- It stops with an error when a followed topic loses a sample in one chunk, or a sample cannot be
  recorded. The recording is then finished, and the player stops naming the recorder. Lower `hypha play
  --chunk` when a topic bursts more than 32 samples in one chunk.
- `--nodes <name>...` (needs `--sync`) makes the first chunk wait until each named node has reported, so
  none is missed. Without it, the first chunk may be acknowledged before a slow node is heard.
- The recorder announces itself to the player, and with `--topics` it answers no chunk before every topic
  named without a wildcard has a publisher, so any start order of player, recorder and nodes records every
  output.

**Times.** MCAP `log_time` and `publish_time` are both the frame's framework send stamp, in nanoseconds
since 1970-01-01: the time its publisher gave it. On host time that is the sender's UTC (a cycle output
carries its step's time, the wall time when the step started, not its trigger sample's stamp); in a
simulated-time replay it is the replayed timeline. A vehicle recording replayed in simulation and
recorded again therefore keeps the vehicle's times: inputs exactly, cycle outputs as their simulated step
time, which differs from the vehicle's by the vehicle step's wake-up delay. A frame without a framework
stamp is dropped and reported. A recording of a looping replay holds each pass with the same times, in
receive order; replay one pass with `--start-utc`/`--end-utc`. Payload sensor stamps are unchanged.

**One channel per stream.** The recorder follows streams, not names.

- A topic with two types, or two versions, is two MCAP channels of one topic, each with its stream's
  schema and its `md5sum` metadata. A stream that joins later (a second version during a migration) is
  recorded from when it was followed, and the recording says how much it sent before ("What a late stream
  sent before it was followed", above). The recorder says `hypha record: following <n> more stream(s), <m>
  in all` on standard error.
- A recording of one type per topic is byte for byte the file the recorder always wrote (MCAP library
  0.25; with several channels the library writes its summary records in hash-map order, so runs differ in
  record order, not content).
- Counts under `Recorded messages:` and loss lines are per stream: `<topic> [<type> (<md5 start>)]` for a
  topic of several streams, `<topic>` for one.
- Under `--sync` a second stream is no failure, since each stream is its own channel: the recorder follows
  a stream that came before a chunk's data before it answers the chunk. A stream that appears during a
  chunk is followed when the chunk is answered, so its first samples are missing: `--sync` fails, naming
  the stream and the count. A stream that refuses the recorder's subscription (other tools hold its
  observer slots) fails a `--sync` recording at once, naming stream and limit; an observer says so on
  standard error and records its other streams.
- By name: `--topics <name>` and `--sync`'s wait for "every `--topics` entry has a publisher" are met by a
  stream of any schema.
- A sample that cannot be recorded (schema unresolved, no framework stamp) is dropped and reported once
  per stream, and fails a `--sync` recording. The recorder never silently writes a stream without a
  definition.

### `hypha play` — replay a recording

```text
hypha play [--input <file.mcap>] [--rate <factor|max>] [--loop] [--clock]
           [--nodes <name>...] [--settle <s>] [--chunk <ms>] [--timeout <s>]
           [--topics <t>...] [--start-utc <timestamp>] [--end-utc <timestamp>]
```

It replays in recorded timestamp order, paced on the recording's timeline scaled by `--rate` (a factor,
or `max` for no pacing). Every replayed topic notifies, so a callback subscriber wakes per sample.

- **Policies.** A topic the player opens first is created with `default` (`maxDepth` 32), so a node that
  starts later and declares another policy for it is refused with `QosMismatch`: start the nodes first, or
  map the type to `default` (cpp-api.md §7). The queue depth is independent of a node's per-step budget. A
  topic whose service a publisher opened keeps that service's queue (8 by default). A subscriber cannot
  create the service: it waits until the topic is published, and the player then publishes it with 8 and
  says so.
- **Several streams.** Two channels of one topic (two types or versions) replay on two streams: one
  publisher per topic and MD5 (two channels of one MD5 share it), so a typed subscriber receives exactly
  its version's frames, in order. Under `--clock` chunk and queue rules count each stream alone: a chunk
  is split before the message that would overflow the smallest subscriber queue of its stream (`hypha
  play: chunks shortened on '<topic>' [<type> (<md5 start>)]`), and published counts are per stream. By
  name, on the cautious side: `--topics` selects every stream of a name; the recording's own `/clock` is
  skipped under `--clock` whatever its schema; `/clock` already published by any process, and a replayed
  name another process publishes in any schema, are refused or reported as below.
- **Start checks.** A topic that cannot be published (a type with no resolvable MD5, a refused publisher)
  refuses the start: each is listed on standard output as `'<topic>' cannot be published: <reason>`,
  nothing is published, and the player exits 1. A replayed topic another process already publishes is
  reported once on standard error, `hypha play: <topic> already has a live publisher; replayed data will
  mix with it`, and the replay goes on; this check runs once, at start.
- **Read errors.** A read error stops the replay where it occurs and exits 1, naming the recording and,
  when the reader gives one, the file offset of the chunk it was reading. A recording that cannot be read
  in full does not open.
- **Complete files only.** The player maps the file into memory, so never replay a recording `hypha
  record` is still writing, and let nothing change or truncate a file during a replay.
- **No skipping.** Every message is published; a replay that cannot keep its schedule runs late, and the
  effective-rate line says what set the rate (`replay-flow-control.md` §6.2). The player serves its
  executor while it waits, so the schema records (republished as hypha schema servers) answer through the
  replay, and late subscribers and the ROS 1 bridge resolve each frame.

**`--clock`** also publishes `/clock`, so nodes started with `HYPHA_USE_SIM_TIME=1` follow the
recording's time. One player per domain: it refuses to start while another process publishes `/clock`
(a ROS clock bridged onto the bus counts).

- **Chunks.** The player sends the recording in chunks of `--chunk <ms>` of data time (default 100): the
  chunk's messages, then `/clock` for its end, then a wait until every participant reported on
  `/clock_progress` that it ran that chunk's deadlines. A participant is a node that publishes progress,
  or a `hypha record --sync` recorder.
- **Start.** `--settle <s>` (default 2) starts the replay once no new node has reported for that long.
  `--nodes <name>...` adds required nodes: the start waits for them too and fails after 30 s if one is
  missing. Unnamed nodes that report take part too. A wait over 1 s prints the awaited nodes.
- **Stalls.** A participant whose clock does not rise for `--timeout <s>` (default 5), quiet or not, stops
  the replay with exit 1, and the error names every awaited node and what it did
  (`replay-flow-control.md` §6.1).
- **Pacing.** `--rate` paces each message on the wall clock; `max` waits only for the participants. A
  report wakes the player at once (no fixed poll); an unanswered `/clock` is resent every 200 ms.
- **Loop and restart.** `--loop` and the restart key start a pass again from the window's start, which
  sends `/clock` back and restarts the participants: no node rewinds in place. The player sends the first
  `/clock` value once, waits until every participant has left the bus (a C++ node on `hypha::runNode`
  exits 75 and its `hypha launch` supervisor restarts it), reopens its publishers, and waits for the same
  participants to rejoin before sending data. A `--sync` recorder stays through it. A participant still on
  the bus, or not rejoined after 30 s, stops the replay; a node with its own `main`, or a Rust node, does
  not restart itself (`replay-flow-control.md` §4.4).
- **Effective rate.** At the end the player prints the data time over the wall time and the participant
  that set it: `effective rate 0.98x: 12.300 s of data in 12.550 s; set by '<node>' (last to answer
  ...)`, or says that `--rate` set it.

**A time window.** To investigate a report at 12:00 UTC, select a surrounding window:

```sh
hypha play --input incident.mcap --clock \
  --start-utc 2026-10-08T11:59:30Z --end-utc 2026-10-08T12:00:30Z
```

Start is inclusive, end exclusive. Timestamps need `Z` or an explicit offset; offsets are normalized to
UTC. The reader uses the MCAP index to select the window. An MCAP viewer such as Foxglove Studio can also
search a recording by UTC. With `--clock`, playback starts `/clock` at the selected UTC start, advances it
at each chunk boundary, and sets each message's UTC before publishing its payload. Recorded `/clock`
payloads are left out so they cannot replace that timeline. Select enough data before the incident to
initialize application state, with the required input topics. Payload bytes are preserved.

**Two modes, one rule.** For a system rerun, start the nodes with `HYPHA_USE_SIM_TIME=1`. Use the
lockstep, the default; if the system cannot run in it, set `HYPHA_REPLAY_LOCKSTEP=0` for the whole launch.

- `HYPHA_REPLAY_LOCKSTEP=0` (or `false`), set for the player and every node, is an open-loop playback
  like `ros2 bag play`. With `--clock` the player prints `hypha play: open loop (HYPHA_REPLAY_LOCKSTEP=0):
  /clock and the data follow --rate, and no node is waited for`, publishes each chunk on the `--rate`
  schedule and `/clock` once per boundary, and reads no `/clock_progress`: no start barrier, no
  acknowledgement, no resend, no chunk split for a subscriber queue, no wait for a restart after
  `--loop`.
- `--rate max` is refused in the open loop, since nothing would pace it, and so are `--nodes`, `--settle`
  and `--timeout`. The nodes follow `/clock` and warn of a lost sample instead of failing; a node still
  exits 75 when `/clock` goes back. The result depends on the host and its load, as a ROS playback's does
  (`replay-flow-control.md` §4.6).
- Any value other than `1`, `true`, `0` or `false` refuses the start.

**What a node sees.** Every frame carries its recorded time as its framework send stamp
(`framework_sent_nanos`), `/clock` included, so a subscriber sees the recording's time. `/clock = T`
goes out only after every message stamped at or before T, and the player waits until every participant
ran the deadlines up to T; the rule and its one exception (the first instant of a pass) are in
`replay-flow-control.md` §4.3.

A cycle node on simulated time replays faithfully at any `--rate` the host keeps up with: its steps run
on the recording's deadlines, with the samples recorded before each, and ages are in recorded time. A
step that loses a queued sample (transport overflow or history eviction) fails with `SequenceGap` and
logs `replay not faithful`. A node drains its inputs on every `/clock`, so a queue holds one chunk, which
the player splits to fit. A queued input needs a `perStep` of at least the cycle's period times the
topic's rate; a `Latest` input on a topic only the player publishes needs nothing more
(`replay-flow-control.md` §6.7). Another publisher on the topic is not split into chunks, and one that
keeps sending while the node drains can leave samples queued after the node's report. Rules:
execution-model.md section 11.1.

Observers, such as a recorder or the bridge, are not waited for: a message sent before one connects is
lost to it.

**Keys while playing:** pause (`<Space>`), step one chunk (`<s>`), speed up and slow down (`<+>`, `<->`),
restart (`<r>`), quit (`<q>`). Keys and the progress line need a terminal on stdin and stdout in the
foreground process group. As a background job or a `hypha launch` unit the player takes no keys and
prints plain lines.

### `hypha convert` — a ROS 2 bag into a recording

Turns a ROS 2 bag in rosbag2's SQLite storage (`.db3`) into an MCAP recording that `hypha play` replays,
without ROS (#46). Uncompress a `.db3.zstd` bag with `zstd -d` first.

```bash
hypha convert run.db3 -o run.mcap --msg-path <dir>    # every topic it has a definition for
hypha convert run.db3 -o run.mcap --topic /imu --topic /lidar/points --xyzirc /lidar/points
```

- **Serialization.** Each topic's CDR messages become ROS 1 serialization through its type's ROS 1
  definition, which the recording carries with its MD5, as `hypha record` writes them. Bags recorded
  before rosbag2 embedded definitions hold none, so definitions come from the message paths: each
  `--msg-path` in order, then the SDK's `share/hypha/msg`. A path holds `<package>/<Name>.msg` or
  `<package>/msg/<Name>.msg`.
- **ROS 2 types.** `std_msgs/Header` has no `seq` in ROS 2, so the converter writes the message's index
  on its topic. `builtin_interfaces/Time` becomes `time`, and a negative second fails the conversion
  (ROS 1 `time` is unsigned); `Duration` becomes `duration`.
- **Times.** A message's log and publish time is the bag's receive time.
- **Left out.** A topic with no definition for its type, or not CDR, is left out and listed with its
  reason; one named with `--topic` fails the conversion instead. A message that does not decode fails it,
  naming topic and message.
- **Point layouts.** `--xyzirc <topic>` rewrites an XYZI `sensor_msgs/PointCloud2` (four `float32`) into
  the XYZIRC Autoware 2.1.0 reads: `intensity` and `return_type` as `uint8`, `channel` as `uint16`, still
  16 bytes a point. Intensities are rounded and held to 0 to 255, return type and channel are 0, and
  `row_step` is recomputed. The tool reports the highest intensity seen; if every intensity rounds to 0,
  it says the recording holds no intensity on that scale.

## Running a system

### `hypha launch` — run a set of nodes in one tmux session

```text
hypha launch up     <file.toml> [--session <name>] [--attach]
hypha launch attach <file.toml> [--session <name>]
hypha launch down   <file.toml> [--session <name>]
```

`up` starts each unit in its own window of a detached tmux session, so every node's output is in one
place. An operator connects over SSH, runs `hypha launch attach fleet.toml`, and moves between windows
with `Ctrl-b n` and `Ctrl-b p`, or `Ctrl-b w` for a list. `Ctrl-b d` detaches and leaves the fleet
running. `down` stops every unit and removes the session.

- **Status line.** `hypha` and the session name on the left, host and time on the right, one entry per
  unit, in the Mycelium palette. A unit whose process ended for good shows in the error colour with `✗`.
  These options apply to this session and its windows only; the user's tmux configuration is untouched.
- **Session name.** The file name without its extension (`fleet.toml` gives `fleet`), or `--session`.
  Names hold only ASCII letters, digits, `_` and `-`. `up` refuses a session that exists. `attach` and
  `down` use the file name only and do not read the file.
- **At boot**, start the fleet from the target's init script: `hypha launch up /etc/hypha/fleet.toml`.
- tmux 3.0 or newer is required; there is no mode without tmux.

#### Units file

```toml
[session]
historyLimit = 10000        # scrollback lines per window, 100..1000000

[[units]]
name = "planner"            # also the window name
exec = "/opt/hypha/bin/planner"
args = ["--rate", "50"]
workingDirectory = "/var/lib/hypha"
restart = "on-failure"      # no (default), on-failure, always
restartDelaySeconds = 1.0   # finite, 0 or more: the first delay, doubled up to 30 s
after = ["lidar"]           # window and start order only; no readiness check

[units.env]
HYPHA_LOG_LEVEL = "info"
```

A unit may not be named `hypha_launch_setup`: `up` uses that window while it builds the session. Every
unit starts with the environment of the `up` command (also when a tmux server already runs) plus its own
`[units.env]`. `TMUX` and `TMUX_PANE` are left out, and a variable that is not UTF-8 fails `up`.

#### What runs in a window

Each window runs `hypha launch unit <file> <name>`, a supervisor for one unit. At startup each unit
waits at a tmux barrier until its window has `remain-on-exit`, then runs the supervisor; this avoids
replacing a live pane's PTY on macOS and keeps even immediate failures visible. The node writes straight
to the window, so its scrollback holds all runs.

- **Exit lines.** On each exit the supervisor prints one line, for example `[hypha launch] 'planner'
  killed by SIGSEGV; restarting in 1.0 s (attempt 3)`. A unit that ends for good keeps its window, and
  tmux shows its exit status. A unit that cannot start reports exit code 127 and follows its restart
  policy.
- **Back-off.** The delay starts at `restartDelaySeconds` and doubles per restart up to 30 s (a first
  delay above 30 s stays). A run of at least 60 s (host monotonic time) resets delay and count. After 10
  restarts with no 60 s run the supervisor prints `[hypha launch] '<name>' gave up after 10 restarts with
  no 60 s run` and exits with the node's last exit code, so a node that keeps dying after 59 s is given up
  too. With `restartDelaySeconds = 0` the 10 restarts happen at once.

The node's exit code decides before the restart policy does:

| Exit | Meaning | `hypha launch` |
|---|---|---|
| 0 | clean stop | restarted only under `always`, counted |
| 78 | configuration refused at start | never restarted, under any policy; `hypha launch unit` exits 78 |
| 75 | simulated clock went back | restarted at once under every policy, `no` included; not counted, no delay. The new run gets `HYPHA_REJOIN_AFTER_NANOS`, the supervisor's steady time when it saw the exit |
| 70, other, signal | runtime failure | restarted under `on-failure` and `always`, counted |

A systemd unit that runs a node directly gets the same rules with `RestartPreventExitStatus=78` and
`RestartForceExitStatus=75`.

**Signals.** Each node runs in its own process group, so `Ctrl-c` in a window reaches only the
supervisor, which stops that unit without a restart. SIGTERM, SIGINT and SIGHUP to the supervisor send one
SIGTERM to the node's process group (the node and its children), then SIGKILL after 5 s. `down` sends
SIGTERM to every supervisor, waits up to 10 s, SIGKILLs a supervisor still alive, then removes the
session.

On Linux a node gets SIGTERM when its supervisor dies, even by SIGKILL (for example from the OOM killer);
its children get nothing then, and a node that ignores SIGTERM survives. The supervisor watches the node
through a pidfd and reaps it only after its last signal, so a signal never reaches a reused process id.
macOS has neither: a supervisor killed by SIGKILL leaves its node running.

#### Limits

- **The fleet depends on the tmux server.** If it stops, every window gets SIGHUP and every node stops.
  `tmux kill-session` does the same, without `down`'s ordered stop.
- **Log output can block a node.** A node writes to a terminal, and one tmux thread reads all windows. A
  node that logs faster than tmux reads blocks in `write()`. A slow attached client does not cause this.
  Do not log at a high rate on a hot path.
- **C and C++ stdio is line-buffered on a terminal**, so a `printf`-heavy node makes more write calls
  than with a pipe or file.
- **Scrollback uses memory per window.** Set `historyLimit` for the target's RAM.
- **A node that calls `setsid` or `setpgid`** leaves its process group, and the supervisor's group
  signals miss it.
- **A node does not own the terminal.** It runs in a background process group of its window and gets no
  keys: a terminal read stops it with SIGTTIN, or fails with `EIO` if it ignores or blocks that signal.
  Output is unaffected. `hypha play` as a unit takes no keys and prints plain lines.

#### Measured cost

macOS 26.6, Apple M1 Pro (10 cores), tmux 3.4, debug build of `hypha launch`, 20 `fluid_particle` nodes
(`--count 20 --seed 1`). CPU is the CPU time used in a 15 s window, as a percentage of one core. The rate
is the last `hypha topic hz /fluid/p/0` report.

| Run | Particle CPU (sum) | `hypha launch unit` CPU / RSS (sum) | tmux server CPU / RSS | `/fluid/p/0` rate |
|---|---|---|---|---|
| Direct, no tmux | 15.1 % | — | — | 19.998 Hz |
| `hypha launch up` | 15.3 % | 0.0 % / 49.9 MB | 0.0 % / 3.9 MB | 20.003 Hz |
| `hypha launch up` + one unit running `yes` | 4.7 % | 0.0 % / 38.1 MB | 99.5 % / 23.5 MB | 20.013 Hz |

- Against the direct run, `hypha launch up` changed particle CPU by 0.2 percentage points and the rate by
  0.005 Hz. The supervisors used no measurable CPU. The cost is 49.9 MB of supervisor memory and 3.9 MB
  for the tmux server.
- With a unit that writes lines without limit, the tmux server used 99.5 % of one core and `yes` only
  5.9 %: the writer was mostly blocked in `write()`. The particles do not log, and their rate stayed at
  20.0 Hz. Particle CPU was 4.7 %; this measurement does not show why it is lower.
- `fluid_particle` does not exit on SIGTERM, so `hypha launch down` took 5 s, the supervisor grace
  period, and the supervisors stopped the particles with SIGKILL.

### `hypha param` — inspect a configuration file

```text
hypha param dump <file>                every scalar key=value pair, sorted
hypha param get <file> <key>           one value, or exit 1 if absent
hypha param validate <file>            prints "<file>: valid", or the refusal and exit 1
```

All three load the file as a node does: its `include` files, `${NAME}` references, relative paths, and
the refusal of a key two files set. A file `validate` accepts is one a node can load; a refusal prints the
node's message after `hypha param: `. Whether the node reads every key is checked only when it starts.
The tool does not touch the bus or write the file: edit it by hand, so comments and key order stay. There
is no live parameter server; parameters are read at node start-up.

### `hypha clean` — remove the shared memory of domains nobody uses

```text
hypha clean --domain <prefix>   every domain whose name starts with <prefix>
hypha clean                     the domain of HYPHA_DOMAIN, the domain "none" when unset
```

A process that ends without cleanup (a crash, a kill) leaves its domain's iceoryx2 resources behind, and
a segment made by another hypha build or with other limits fails the next open
(`docs/engineering-traps.md`). `hypha clean` removes every resource of a domain nobody uses: the
shared-memory segments (`/dev/shm` on Linux, the objects `/tmp/iox2*.shm_state` names on macOS) and the
services, node files, sockets and schema files under `/tmp/iceoryx2`. It matches names inside the
per-node directories `nodes/<id>/`, which several domains share, and removes such a directory only once it
is empty. It kills nothing.

- **What keeps a domain.** A live node, as iceoryx2's node monitor reports it: the lock a live process
  holds on its node's monitor file, which the kernel releases when the process ends. A node of another
  user, or one with inconsistent files, also keeps its domain. Under `--domain`, a domain is also kept
  while its run lives: the process whose id follows the prefix in the name, so a test domain `t_<pid>_<n>`
  stays between the tests of a running suite.
- **Blind spots.** A process that holds a resource without a node of the domain is not seen. A node in the
  middle of its start is not yet listed, so cleaning a starting domain can remove what that node just
  made.
- `hypha clean` without `--domain` never touches another domain, even one whose name starts with this
  one.

It prints a line per domain removed (with its resource count), per domain kept and why, and per resource
it could not remove, then a summary:

```text
hypha clean: removed domain t_4711_3 (21 resources)
hypha clean: kept domain t_5120_1: node sensor_driver of pid 5133 is alive
hypha clean: of the domains under 't_', removed 1, kept 1, failed 0
```

Exit codes: 0 when nothing removable is left (every covered domain removed, or nothing to remove), 2 when
it kept a domain in use (it may have removed others), 1 on a failure (a refused command line, a resource
it could not read or remove). The printed lines tell a removal from nothing to remove. A caller decides
what a kept domain means: `test.sh` and `examples/check.sh` accept 2, since a kept test domain belongs to
another live run, and fail on 1.

To wipe every domain of the host and kill the processes that still hold a resource, the repository keeps
`./clean-shm.sh [--force]`, which needs `lsof`.

## Testing communication

### `hypha intercept` — rewrite fields between a producer and its consumers

```text
hypha intercept [--name <backend>] [--config <path>]    defaults: default, ./intercept.toml
```

A headless man-in-the-middle relay. Each interception reads its `input` topic, rewrites fields of each
frame, and publishes the result on its `output` topic under the input's type identity (`data_type`,
`md5`, definition), keeping the producer's framework send time. It needs no generated type: the schema
is resolved from the producer at run time. Point the consumers at `output`, or the producer at `input`, to
put the relay between them. The backend runs as node `hypha_intercept_<backend>`, with or without a
connected `hypha top`; the Intercept tab is a frontend for it. Inputs wake the relay on each sample.

**The config file** holds the desired state. The backend reloads it on change; `hypha top` writes it
atomically (temporary file, then rename). All values are text, parsed against the field type when
applied, so the file does not depend on a schema:

```toml
revision = 7                       # bumped by every writer; echoed once applied

[[interception]]
name   = "battery"                 # unique, [a-z0-9_]+
input  = "/battery/raw"
output = "/battery"
armed  = true                      # false = bypass: forward unchanged (default true)

[interception.overrides]
"cells[2].voltage" = { value = "0.0" }
"status"           = { set = "0x0002", clear = "0x0008" }   # either defaults to "0"
"serial_number"    = { value = "ZZZ\\x00" }                 # \xNN and \\ escapes
```

- Paths are `a.b`, `arr[3].c` and `data[2]`. Integers take decimal, `0x` or `0b`, with an optional `-`;
  `time` and `duration` take `sec.fraction`.
- A string override may lengthen the message: the output payload grows. No override changes an array
  length, and byte arrays (`uint8[]`, `byte[]`, `char[]`) cannot be overridden.
- A config that does not parse, or holds an invalid or duplicate name, an interception that reads its own
  output, two interceptions writing one output, or an override that does not parse, is not applied: the
  previous one stays, and `config_error` says why.

**The applied state** is a latched `std_msgs/String` on `/hypha/intercept/<backend>/state`, published on
every change and at least every 500 ms. Its `data` is a TOML document:

```toml
backend = "bench"
config_path = "/abs/path/intercept.toml"
applied_revision = 7
config_error = ""                  # non-empty: the last load failed

[[interception]]
name = "battery"
state = "live"                     # resolving | live | schema_changed | error
data_type = "sensor_msgs/BatteryState"
received = 50412                   # frames read
modified = 50410                   # frames with an override written
published = 50412
patch_failed = 0                   # frames with an override that could not be written
unresolved_dropped = 0             # frames before the schema resolved
last_error = ""
```

Nothing is invented and nothing is hidden:

- Frames that arrive before the input's schema resolves cannot be republished; they are dropped and
  counted.
- A frame whose payload does not match the schema is forwarded unchanged and counted in `patch_failed`.
- An override whose path one message lacks (`cells[5]` when `cells` has three elements) fails that path
  only; the other overrides apply.
- When the producer restarts with another schema, the interception stops forwarding (`schema_changed`)
  rather than patch at stale offsets. Any later change of the file rebuilds a stopped interception on the
  current schema (in `hypha top`, press `a` twice).

## Measuring the framework

### `hypha load` — latency, loss and memory under a defined load

```sh
hypha load examples/load/smoke.toml --report smoke.json [--recording smoke.mcap]
```

A workload file names worker processes, the topics each publishes and subscribes to, the run's times,
the recorder and the pass limits. `hypha load` starts one worker per `[[process]]` (this executable in a
hidden worker mode), leads them through the run, samples their memory, writes one JSON report and prints a
short summary.

- **Workers** are ordinary bus nodes, `load_<process>`, so the workload decides which publishers and
  subscribers share a process. They use the Rust SDK; the runtime below the C ABI is the one a C++ node
  uses, and the thin C++ layer is not in the measured path.
- **Domain.** The run uses its own domain, `load_<pid>` (`<HYPHA_DOMAIN>_ld<pid>` if the caller set
  one), so it cannot touch a running stack. It removes that domain's shared memory at the end and names a
  domain it had to keep.
- **A killed coordinator.** SIGKILL to the coordinator ends no child. On Linux each worker and the
  recorder get SIGTERM from the parent-death signal, and a worker also stops when its stdin closes; on
  macOS the recorder keeps running. Stop it by the pid the coordinator printed (`hypha load: the recorder
  runs as pid N`), then run `hypha clean --domain load_`.

Exit codes:

- `0`: every limit passed. `1`: a limit failed.
- `70`: the run itself failed: an invalid command line or workload, a worker that died, a worker silent
  for more than three sample intervals (at least 10 s), a start-up or end phase over 30 s, an interrupt,
  or a recorder that died, was ended by a signal, exited with a code other than 0 or 1, or left an
  unreadable recording.
- `hypha record` exits 1 when it lost samples. With a readable recording that is no failed run: the
  limit `max_unrecorded` judges the loss, and the report's `recorder.exit_code` says how it ended.

The report path is removed at the start, and the report is written beside it and renamed into place, so
the path holds this run's whole report or nothing. A failed run still writes its report when it can, with
`"outcome": "run_failed"` and the reason; an invalid workload writes none. The summary prints after the
report is written; if the report cannot be written, it prints `hypha load: RUN FAILED` and exits 70.

#### Workload

| Key | Required | Meaning |
|---|---|---|
| `[run] duration_s`, `warmup_s`, `cooldown_s` | yes | The measuring window; the time before it that no statistic counts; the time after it in which publishers keep sending, so a lost last frame of the window shows as a gap |
| `[run] sample_interval_s` | yes | The period of the time series and of the workers' heartbeat |
| `[run] bounded_memory` | yes | Runs the workers as a bounded deployment (`HYPHA_BOUNDED_MEMORY`) |
| `[recorder] enabled`, `compression` | no (off, `lz4`) | A real `hypha record` child that records every topic of the run |
| `[[process]] name` | yes | A worker, `[a-z][a-z0-9_]*`; `recorder` is the report's name for the recorder and is refused |
| `[[topic]] name`, `bytes`, `rate_hz`, `publisher`, `subscribers` | yes | One topic: message size (at least 20 bytes, the probe header), rate (messages per second, or `"max"` for back to back), publishing process, subscribers |
| `[[topic]] burst = { count, every_s }` | no (none) | `count` extra messages back to back every `every_s` seconds |
| subscriber `process` | yes | The process it runs in |
| subscriber `depth`, `work_us` | no (0, 0) | Queue depth (0: the deepest the topic's service admits, 32 under the `default` policy a load publisher creates), and busy work per sample, to make a slow reader |
| subscriber `limits` | no (the run's) | Its own limits, replacing those of `[limits]` |

- Refused: an unknown key, a process with nothing to do, a topic with two subscribers in one process, and
  a topic with more than three subscribers counting the recorder (a topic admits three subscribers that
  declare no policy, as workers and recorder do).
- A process is refused when its subscribers' full queues times their `work_us` reach the heartbeat bound:
  one `WaitSet` turn would hold its heartbeat, and the message names both numbers. A slow reader shares
  its process's `WaitSet` and holds up every other callback there, so give it a process of its own.
- Each message is a `hypha_load/Probe` (`uint64 sent_nanos`, `uint64 sequence`, `uint8[] fill`), so a
  recording of the run decodes like any other.

#### Limits

Every key is optional; an absent key is not checked. A limit whose value was not measured fails.

| Key | Checked on | Fails when |
|---|---|---|
| `latency_p99_us` | each subscriber | the 99th percentile over the window exceeds it |
| `max_interval_latency_p99_us` | each subscriber | the 99th percentile of any one sample interval of the window exceeds it, taken from every interval as it ends |
| `max_loss` | each subscriber | frames lost exceed it: the framework's missing sequences from the window's start to the stop (cool-down included), or the window's messages not received if that is more. A window message still queued at the stop counts as lost |
| `loss_accounted` | each subscriber | the framework's missing-sequence count differs from the harness's own count of skipped probe sequences, or a probe came out of order or was no probe; it reports that difference plus those probes, against a limit of 0 |
| `max_refused_publishes` | each publisher | more publishes than this were refused over the run |
| `max_publish_call_us` | each publisher | the window's longest publish call exceeds it |
| `max_missed_periods` | each periodic publisher | more of the window's deadlines than this were missed; a `"max"` topic has no periods and no result |
| `max_unrecorded` | each topic | more of the messages the run sent than this are missing from the recording; without the recorder it is not measured and fails |
| `max_rss_growth_bytes` | each process | its largest resident memory from the window's start to the end of publishing exceeds that at the window's start by more |
| `max_shm_growth_bytes` | the domain | its written shared memory grows by more over the same span |

#### How it measures

- **Latency** is send call to callback entry. The publisher reads `CLOCK_MONOTONIC` just before its
  publish call and writes it into the probe; the subscriber's `WaitSet` callback reads the same clock
  first. All processes of a host share that clock, and it never steps. A histogram of 3,776 fixed
  log-linear buckets (64 per power of two, at most 1/64 or about 1.6 % wide) keeps it; a quantile is its
  bucket's upper bound, clamped to the exact maximum, so it never reads low and at most about 1.6 % high.
  Each publisher also has a histogram of its publish call's duration.
- **Loss** is the subscriber's missing-sequence count (`RawSubscriber::counters`), read at the window's
  start and at the stop. Beside it the harness counts gaps in the probe's own sequence, and messages sent
  and received in the window by send stamp. A refused publish moves the probe's sequence like the
  framework's (the framework takes a sequence number for a publish its pool or send refused), so it is a
  gap for both.
- **Missed periods.** A publisher keeps absolute deadlines `start + k * period`. When it gets to send, it
  sends one message for the latest passed deadline and counts the earlier ones as missed, as a timer does;
  it does not catch up. A deadline counts when it lies in the window, whenever the publisher noticed it,
  including window deadlines never reached because publishing ended. A burst adds messages without
  deadlines of its own, but can make the next deadline late. A sleep that wakes a period late (a loaded
  host, macOS timer coalescing at 1 kHz) is a missed period of the host, not of the harness.
- **Memory.** The coordinator reads each process every sample interval. Linux gives `VmRSS` and
  `RssShmem` from `/proc/<pid>/status`, and the domain's written shared memory as the allocated blocks of
  its `/dev/shm` objects. macOS gives only the resident size from `ps`; the report marks shared-memory
  values `null` and says why under `not_measured`.
- **The report** holds the host (name, OS, kernel, CPUs, build profile), the parsed workload and its MD5,
  the domain, each topic's publisher and subscriber results, each process's memory, the recorder's counts,
  a time series and every checked limit. The series keeps at most 512 points; when full, neighbouring
  points merge, keeping the larger values. Neither harness nor report grows with the run's length.

A latency figure is a property of the host that produced it: the report names the host, and the test
suite asserts no latency or speed (#64).

#### The rated load

`examples/load/rated.toml` is the rated load of NFR-04 (46 topics, 72.5 MB/s, 9,960 messages/s, MB =
10^6 bytes) with the limits of NFR-03 (p99 below 500 µs, send call to callback entry) and NFR-04 (zero
loss for readers that keep pace, every loss accounted, no refused publish), and no missed period, so the
run offered the load it names. The requirement gives only totals; the file's per-topic split is
constructed, and `load_workload.rs` recomputes the totals from it.

```sh
./build.sh
scripts/rated-load.sh [report.json]    # default target/rated-load.json
```

The script runs `target/release/hypha` without building it and forwards the exit status of `hypha load`.
`scripts/qualify.sh` runs it after the C++ tests as the measurement `rated_load` of the qualification
report: failing limits turn nothing red, a failed run (exit 70, or no report) does, and no requirement
takes it as evidence.

A run counts for NFR-03 and NFR-04 only on the reference platform, which is not defined yet. Any other host is a development host, and its report
measures that host. Read it so:

- `host`: `os` and `os_release`, `arch` and `cpus` say which machine was measured. Other load on it (a
  running stack, a build) is not recorded and shows as latency and missed periods.
- `host.build`: `release` or `debug`. A debug build measures unoptimised code; its latency says nothing
  about a release.
- `not_measured`: what this host could not measure (on macOS the shared memory), each with its reason.
- A publisher's `missed_periods` above 0 means the rated load was not fully offered: that run's latency
  and loss belong to a lower load.

#### The overload run

`examples/load/overload.toml` is a sustained overload for NFR-05: a topic sent back to back to a slow
reader, a 1 MiB topic to a slower one, bursts into a short queue, and two 100 Hz control topics between
two processes that share nothing with the flood, for a 10 s warm-up and a 60 s window. Its limits are
counted or bounded facts, not speeds:

- `max_refused_publishes = 0`: a slow reader makes the publisher discard that reader's oldest sample and
  never empties its pool, so no publish is refused. A publish call cannot block; its duration is a speed
  and is not limited.
- `loss_accounted` on every subscriber, the flooded ones included, and `max_loss = 0` on the control
  readers, which keep pace (a queue of 32 at 100 Hz loses a sample only if its reader is not scheduled for
  320 ms).
- `max_rss_growth_bytes` per process and `max_shm_growth_bytes` for the domain, both 1 MiB: memory stays
  flat after the warm-up. Every publisher is bounded, and `bounded_memory = true` runs the workers as a
  bounded deployment, the condition of NFR-05. The warm-up is long enough for every pool slot of every
  topic to be written once (the file's header has the arithmetic). The shared-memory limit is unverified on
  Linux.

`examples/load/isolation-latency.toml` is the same workload with a latency limit on the control topics
(p99 below 500 µs); `load_workload.rs` checks nothing else differs. Like the rated load, it measures the
host.

```sh
./build.sh
scripts/overload.sh overload [report.json]            # default target/overload.json
scripts/overload.sh isolation-latency [report.json]
```

`scripts/qualify.sh` runs both after the rated load, as the measurements `overload` and
`overload_latency`: failing limits turn nothing red, a failed run does. The overload run is no evidence of
NFR-05 until a Linux run confirms its limits; it then becomes the reference `run:overload`.

Only Linux measures written shared memory. On macOS that value is `null`, `not_measured` names it, and
`max_shm_growth_bytes` fails as not measured, so the run exits 1 by design there. Read the counted limits
from the report's `limits` list instead: every entry but `max_shm_growth_bytes` with `pass: true`. On
macOS resident memory includes shared pages, so `max_rss_growth_bytes` there also holds the shared memory a
process touched.

## Bridging

### `hypha_ros_bridge` — connect ROS 1 and hypha topics

Bridges ROS 1 Noetic topics and same-name hypha topics in one process, or the routes of an explicit
manifest. It carries message bodies as opaque bytes. A bridged ROS 1 sample's transport age starts at the
bridge, because ROS 1 sends no send time. Options, manifest format and limits:
[`apps/hypha_ros_bridge/README.md`](../apps/hypha_ros_bridge/README.md).

### `hypha foxglove` — view the bus in Foxglove Studio

Serves the bus over a WebSocket in the Foxglove protocol, so topics appear as channels in Foxglove Studio.
Run it, then connect Studio to the address it prints.

- **Late clients.** It replays each channel's last value to a client that subscribes between publishes, so
  a slow topic is visible before its next message. For `/tf_static` it keeps one message per set of
  (parent, child) frame pairs, so a late client gets every broadcaster's transforms and a re-stamped
  message replaces its predecessor.
- **A display, not a recorder.** It drops data rather than fall behind. Each topic is read through a queue
  of 8 samples, so a fast topic is sampled, not carried whole. A sample that waits longer than
  `--max-latency` (500 ms by default) for a slow client is dropped, and a client whose socket is full is
  not disconnected.
- `--topics` takes names or patterns, as `hypha record` does, and is also the subscription set (with
  `/clock`), so the bridge reads only what it shows.
- Time frames from `/clock` go out at most every 20 ms. For a replay at a high `--rate`, add `--max-rate
  <Hz>`: it limits each topic in wall time, whatever the replay rate.
