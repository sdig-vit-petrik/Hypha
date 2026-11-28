# Command-line tools

Twelve tools ship with hypha, as one executable: `hypha <tool>` with the tools
`topic`, `top`, `node`, `record`, `play`, `convert`, `intercept`, `foxglove`, `launch`,
`param`, `clean` and `load`; `hypha --help` lists them. Two shorter names run the same
executable. `hy` is `hypha`: `hy play --input run.mcap` is `hypha play --input
run.mcap`. Each tool has a link of its own, `hy<tool>`: `hytopic`, `hytop`,
`hynode`, `hyrecord`, `hyplay`, `hyconvert`, `hyintercept`, `hyfoxglove`, `hylaunch`,
`hyparam`, `hyclean` and `hyload`, so `hyplay --input run.mcap` is the same command again. The
installation makes `hy` and these links; the old names `hypha_<tool>` are gone.
Usage and messages name the tool as `hypha play: ...` whatever name started it. In
CMake, `hypha::<tool>` (`hypha::topic`, ...) runs a tool's link. `hypha_ros_bridge`
and `hypha_msggen` stay executables of their own.

The tools are ordinary bus nodes: each one joins the
bus through the framework, and none of them reaches around it to the transport.
`hypha convert` and `hypha param` work on files and do not join the bus, and
`hypha clean` removes a domain's resources without joining it.

`hypha top`, `hypha topic`, `hypha node` and `hypha record` without `--sync` join
as CLI tool nodes, `_hyphacli_<tool>`: any number of each runs at once in one
domain, `hypha top` does not show them, and the next node that starts reclaims one
that died. `hypha play`, `hypha record --sync`, `hypha intercept` and
`hypha foxglove` publish or answer under a name of their own, so a second one
with the same name in a domain is refused.

Build them with `./build.sh`; `hypha` lands in `target/release/`, and the build
tree links `hy` and the `hy<tool>` names to it in `build/apps/tool-links/`.

Run any tool with `--help` for its exact options. This page is a map, not a
manual — the binaries are the authority on their own flags.

Every terminal surface the tools draw — `hypha top`, the `hypha launch` tmux
session and the `hypha play` progress line — uses one palette, Mycelium, defined in
`apps/hypha_tools/src/theme.rs`. It needs a
truecolor terminal and no special font. State is shown in form as well as colour
(`✗`, `[PAUSED]`), so each view stays readable without colour.

---

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
```

`list` reads the transport's service listing once and returns at once: it listens to
nothing. A topic is listed from the moment a publisher, or a subscriber that declared its
policy, has opened it, whether or not a sample has been sent, so a node whose topics are
silent shows its topics. A line names the topic and its message type,
`/odom  [nav_msgs/Odometry]`; a topic that carries two types, or two versions of one type, is
two lines, each with the first eight hex digits of its MD5, `/odom  [nav_msgs/Odometry
(cd5e73d1)]` (topic name overloading, cpp-api.md §5). A stream whose publisher gave no
definition shows its schema hash, `[? (0123456789abcdef)]`. `-v` adds what the listing counts
for free, the endpoints of each stream, `/odom  [nav_msgs/Odometry]  1 publisher, 2
subscribers`; a crashed publisher is counted until its node is reclaimed. A bus with no
topic prints nothing on standard output and `no topics on the bus` on standard error. The
listing opens each stream for an instant, which takes one node slot of the stream while it
lasts (interface-specification.md), and reads every service of the domain, so it takes
longer on a bus of many streams. `hz` and `bw` cannot measure a topic that is not currently publishing.

**A topic of several streams.** A topic name carries one stream for each schema, and
`echo`, `hz` and `bw` read all of them: each stream is followed on its own, and one that
joins the name after the tool started, a second version in a migration, is followed from the
moment the tool learns of it (it hears the graph event of a publisher or subscriber that
comes or goes; see "Following streams" below), and says on standard error how many samples
the stream had sent before. A tool started before any publisher waits, says so once, and follows
whichever stream appears first. While a name has one stream the output is what it always
was; once it has more, each stream says which it is:

- `echo` prints a `# <type> (<md5 start>)` comment line after the `---` of each message.
  `echo -p` prints one table, so it refuses a name with several streams: it prints nothing,
  names the streams with their type and MD5 start, says to pick one with `--type` or
  `--md5`, and exits 1. One stream, selected or alone, prints as it always did.
- `hz` and `bw` keep one window for each stream and print a title line,
  `<topic> [<type> (<md5 start>)]:`, before each stream's figures.

`--type <type>` follows only the streams of that message type, and `--md5 <prefix>` only
the streams whose MD5 starts with those hex digits (not case sensitive); together, the
streams that satisfy both. A flag needs the stream's schema: a stream whose publisher gave
no definition matches none, and is followed once a publisher that gave one joins it. When the
flags match no stream of the name, the tool lists the streams the name has (a stream without a
definition by its hash) and waits. One
stream selected is printed as a topic of one stream. The flags are the same on all three.

`echo -p` prints one CSV row for each message, as `rostopic echo -p` does. The
first line is the header: `%time` and the column names. `%time` is the frame's
framework send stamp in nanoseconds: the time its publisher gave it, UTC on host time.
For a cycle output that is the step's time. On host time it is the wall time at the
start of the step, so an output of a triggered step is stamped later than the trigger
sample that caused it, and the sample's stamp is not carried by it. On simulated time
it is the time the step ran at, which for a triggered step is the trigger sample's
stamp T. For a replayed message it is the recorded time. A nested field is `outer.inner`, an array element is
`field0`, `field1`, and a `time` is `field.secs` and `field.nsecs`. A `bool` is
`1` or `0`. A float has its full precision. A string is written as it is, with
no quoting, so a color escape code in a debug string reaches the terminal. A
string that contains a comma or a line break breaks the row. The header comes
from the first message and is not printed again. A message that cannot print
(schema not resolved, payload does not match the schema) is reported on stderr
and gives no row. `echo` refuses an unknown option or a `--count` without a
number.

To publish a debug string only while someone reads it, check the publisher's
`subscriberCount()` first (C++, `docs/cpp-api.md` section 4).

`echo` resolves the compact frame hash against the publisher's retained schema
record, then prints named fields rather than bytes. The record can reach a new
reader a moment after the first frame, so `echo` waits a short time and
retries. When the schema is not available it says so instead of guessing, and
a payload that does not match its schema ends with a line that says so. The
decoders follow at most 64 levels of nested messages (`MAX_NESTING_DEPTH`, the
same limit definition resolution in `hypha_msgdef` applies): a recursive or deeper
definition ends the same way instead of aborting the tool, and so does a definition
that expands to more than 65 536 nested messages (`MAX_EXPANDED_MESSAGES`), before
any message is decoded.

Every tool that must name a message type resolves the frame's schema hash the
same way (`hypha::SchemaIdentityCache`): `hypha topic`, `hypha top`, `hypha record`,
`hypha foxglove` and the ROS bridge. There is no compiled-in fallback. A
frame carries only the hash, so a tool that skips this step cannot report a type
name, an md5 digest, or a definition.

`hz` measures the cadence from the timestamp each publisher wrote into the
frame, not from when this process happened to read it, so a slow reader does
not distort the reported rate.

**`delay`: how long a node takes from an input to its output.** `hypha topic delay <input>
<output>` is the `rostopic delay` of a node that has no header to compare: it follows both
topics and, for each frame of the output, takes the time since the newest frame of the input
sent at or before it. Both times are the host monotonic send stamps in the frame headers, so
it needs nothing from the node, decodes no payload (a 1.6 MB cloud costs what an `Int32`
costs: the sample is dropped after its header is read) and does not depend on the reader's
speed. Once a second it prints the last `--window` outputs (default 100):

```text
$ hypha topic delay /points /points_filtered
delay: median 1.412 ms, p99 2.903 ms, max 3.104 ms (min 0.912 ms) over 100 outputs
```

For a triggered node it is the time from the trigger sample to the output, the step's own
time included: a cycle output takes its transport stamp when the step sends it, after the step
ran. For a rate node it is the age of the newest input when the output leaves, so it holds the
wait for the next step (half a period at the median). A node that reads several inputs has
one delay for each: name the input whose delay is wanted. The tool keeps the input stamps of
the last 5 s (at most 65 536); an output sent before every one of them has no delay, and is
counted in the line: `, 3 outputs had no input before them`. Every stream of a topic name
counts, and `<input>` and `<output>` must differ. Both topics must be sent on this host: the
stamps of two hosts are not one clock. A replay measures the real time the node takes, not
the recorded one. The step times of the node alone, with no second topic, are in `~/state`:
`hypha top` shows them (below).

### `hypha top` — live bus monitor

```
hypha top [--intercept-dir <dir>]
```

`--intercept-dir <dir>` names the directory the Intercept tab may read and write config
files under; the default is the working directory when `hypha top` starts. A config path
the backend names on the bus is used only when, with `..` and symbolic links resolved,
it lies inside that directory. Any other path, a relative one among them, is refused:
the tab reads nothing from it and writes nothing to it, and the status bar shows
`refused: <path> is outside <dir>`.

An interactive terminal view: topic rates and bandwidth, a node registry, and a
message inspector. It resolves compact frame hashes against each publisher's
retained schema record before showing the message type and fields. A `/<node>/state` or
`/<node>/introspection` topic makes a node entry only when its frames carry the hash of
that `hypha_msgs` type; a topic of another type with the same name is ignored. A
recording made before `hypha_msgs/NodeState` gained its cycle and counter fields carries
the old hash, so a replay of it shows its nodes without a state. The node details show
one line per cycle (steps, missed periods, the step time in ms as median, 99th percentile
and maximum since the start and over the last 10 to 20 seconds, the last step, overruns,
output drops), the log counters, and per input the words `STAGNANT` and `IGNORED` and the
`DROP/REJ/UNDEC` counts. A topic of several streams is one row for each, each with its type
and the start of its MD5 (`nav_msgs/Odometry (cd5e73d1)`), and one that joins later is
followed by the graph event, not by listing the names again. An input binding shows its
type and MD5, is judged by its own stream, and shows in `OTHER SCHEMAS` what its node found
of the topic's other versions: `not checked` before the node's first check (and for a node
whose default `WaitSet` does not run), `none`, or each version with a live publisher by type
and MD5 start. A version that is published while the input has no publisher of its own is
red, so a mismatch shows at a glance; the node list's `INPUTS` column adds `1 OTHER VERSION`
to a waiting or stale count. Keys: `1` to `5`
select a view, `j`/`k` or the arrows move, `PgUp`/`PgDn` move a page and `Home`/`End`
jump to the ends, `Enter` opens the inspector, `/` searches, `s` changes the sort
(by name first, then bandwidth, rate, messages), `Space` pauses, `q` quits. It exits 0
after a normal quit. When its loop fails, for example when the node registry cannot be
listed, it restores the terminal, prints `hypha top: <error>` on standard error and exits
1. A list longer than the view scrolls to keep the selected row visible. The node list shows
each process's CPU, measured as the CPU time it used over the last second (100 % is
one core, as in `htop`, to a tenth of a percent), and its resident memory; on Linux it
reads the process CPU clock and `/proc`, elsewhere `ps`, whose CPU time counts 10 ms steps, and shows `-` for a process it cannot read. CLI tool nodes (`_hyphacli_*`),
this monitor among them, are not listed. The monitor is one node (`_hyphacli_top`) and
starts no thread. Topics and traffic appear immediately; their schema identities are
filled in later, one lookup per turn of the render loop, which reads the retained
record once and never waits for it. A record not there yet is asked for again after
100 ms, not on every frame, and the topic shows no type meanwhile.

The Intercept tab (`3`) drives a running [`hypha intercept`](#hypha-intercept--rewrite-fields-between-a-producer-and-its-consumers).
It finds the backend by its state topic, edits the backend's config file, and shows
each field's In and Out values from the traffic it already receives; it needs no
type of the intercepted topic. Two columns: on the left the interceptions and the
selected one's details (input and output with their rates, the message type, armed or
bypass, the relay state, frame counters and the newest error); on the right the field
tree of the selected input above the override editor (dec/hex/bin and a tri-state bit
grid for an integer, the value and its bit pattern for a float, a toggle for a bool,
`\xNN` escapes for a string, `sec.nsec` for time). The tree's `ovr` column shows `V`
for a value override, `B` for bits, `S` for an override below that row; a path the
output does not hold shows `–`. Config files are confined to the intercept directory
(see above). The status bar shows `● ARMED` or `○ BYPASS`, the
newest error, `◌ pending` until the backend applied the written revision, and
`⚠ backend offline` when no state arrived for 2 s; the footer lists the keys of the
focused pane. From 110 columns both columns show; from 80 a one-row selector (with the
input's type) replaces the left column; from 60 one pane shows at a time with a
breadcrumb; below 60×12 the tab shows only the size it needs. A resize keeps the
selection and the text being typed.

`n` opens the new-interception modal over the tab. Input, output and name are typed
in that order; output (`<input>/intercepted`) and name (`/battery/raw` →
`battery_raw`) follow the input until typed in. Under a topic field the matching bus
topics are listed with their type, prefix matches first: `↑↓` highlights one, `Enter`
or `Tab` takes it. The input's type and rate show under it. `Enter` with no hint
highlighted adds the interception; `Esc` drops a highlight, then the modal. `d` (or
`Del`) asks to delete the selected interception in a modal: `y` deletes it with its
overrides, any other key keeps it.

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

In the Override pane every printable key is text, so `q` and the digits do not
switch tabs there; `Esc` leaves the pane.

The Graph tab (`5`) draws who publishes to whom, flowing left to right as
`rqt_graph` does, from the latched `/<node>/introspection` manifests. A node shows
its health: `●` ok, `▲` waiting for an input, `✖` degraded, `◌` silent (no
`~/state` for 3 s), `○` dead, `?` no state. `∅` marks a node without a manifest
(Rust and Python nodes, tools such as `hypha play`); it is drawn without edges. CLI
tool nodes are not drawn. `g` switches between nodes
only (one edge per pair of nodes) and nodes with topics (rounded boxes; a topic with
no publisher or no subscriber is grey). `i` shows `/clock`, `/hypha/...` and the
`~/state` and `~/introspection` topics, hidden by default. A wildcard subscription is
drawn to every bus topic it matches. `/` keeps the matching nodes and topics and their
direct neighbours. `↑↓` select, `←→` and `PgUp`/`PgDn` pan, `Enter` opens the selected
node or topic in its tab, `e` writes the graph to `hypha_graph.dot` in the working
directory for Graphviz.

Nodes that reach each other through topics form a loop: two nodes that feed each
other, a longer ring, or one node that reads its own output (`↻`). A loop is a
warning, not an error: it can be intended, but it can also make a system behave in
surprising ways. Loops are listed under the graph and drawn in the warning colour.
The list names the kind. A *feedback* loop passes through a rate cycle, so it runs at a
bounded rate, as a controller and its plant do. A *triggered* loop is closed between
triggered cycles and subscriber callbacks only, so one message can circulate without
bound. A publisher that no cycle output owns counts as fed by every cycle and
callback of its node, so a loop may be called triggered when a rate cycle in fact
bounds it. An edge that closes a loop points
back (`◀`). The manifest is sent once, after setup; an endpoint created later is not
drawn.

### `hypha node` — what is running

```text
hypha node list           every process the node registry knows about, alive or dead
hypha node info <name>    detail for each node of that name
```

The processes of one CLI tool share its name, so `info _hyphacli_topic` can list
several nodes.

Reads the transport's own node registry within `HYPHA_DOMAIN`. `info` shows
the domain and the `/<node>/state` topic for C++ cycle input health; observe it
with `hypha topic echo`. Rust app nodes do not publish that state. `info` lists no
inputs, so it shows no stream or version: `hypha top` does.

Where the backend cannot supply a
detail, the tool prints that it is unavailable rather than inventing a value.

### Following streams

`hypha topic echo`, `hz`, `bw`, `hypha record`, `hypha top`, `hypha foxglove` and the ROS 1
bridge's automatic mode read topics they were not told the schema of, so each reads the
streams that exist when its subscription is made. To follow a topic name through a
migration they all use one follower (`hypha_node::sdk::StreamFollower`): it opens one
subscription for each stream (a topic name and a schema hash) that matches its patterns
and the `--type`/`--md5` flags, and one for each stream that joins later.

It learns of a new stream from the graph event, the one a node's own housekeeping hears:
every publisher and subscriber that is created or dropped notifies it. The tool opens a
listener of the event before its first listing, so no stream falls between the two. A thread
of the tool's own (the SDK creates none) sleeps until the event wakes it and lists the
streams, at most once per 100 ms and at most one fifth of its time (the next listing waits
four times as long as the last took), and after 2 s with no event, in case a notification
was lost (the sender logs a failed one). The tool's loop takes what the thread found and
subscribes to the new streams, which is cheap: a stream is subscribed by its schema hash, with
no listing of its own, so a stream whose publisher gave no definition is read too; its type
is looked up only for a `--type` or `--md5` flag and for a label. A `--sync` recorder lists on
its own thread instead, before it answers a chunk, because it must have every stream there is.

The cost, measured in a release build on a loaded development machine (load average 15): a
pass of the loop with nothing to learn takes 1 µs (4 to 13 µs in a debug build); a listing,
which reads the state of every service of the domain, takes 70 ms with one stream and 120 ms
with 100, on the helper thread, and the first listing that follows 100 streams, with their
subscriptions, 170 ms. A 1 kHz topic recorded for 6 s while the graph is quiet lost 165 to 168
samples to the backstop's listings when the loop listed itself, and 0 (5 runs of 8; 7, 14 and
124 in the others, with a build running beside) with the helper thread. Each followed stream takes one observer
slot of its service for each tool (`RESERVED_OBSERVER_SUBSCRIBERS`, 3): a fourth tool on one
stream is refused, and says `all 3 observer slots of '<stream>' are taken by other tools`.

**What a late stream sent before it was followed.** A publisher's first frame has sequence 1,
so the first sequence a subscriber accepts from a publisher, less one, is the number of that
publisher's frames the tool never saw. The tools count them for a publisher known to have
come after them: every publisher of a stream that a later listing than the first showed, and
the publishers that joined a followed stream since a listing showed one more publisher on it.
They do not count a publisher that ran when the tool started (its history is not the tool's
to record), nor a latched publisher's replayed sample (a latched stream is not judged), and a
second publisher that joins a stream that had one is told from a quiet one by the listing's
count alone. A publisher whose port registered after the tool's subscription of the stream
owed it every frame, so what it sent before the first frame the tool accepted is a loss,
reported as `lost` and failing the recording, not as sent before it was followed
(`communication-integrity.md` §3). `hypha record` prints `hypha record: <stream>: <n> samples were sent before it
was followed` as it finds them and `<stream> sent <n> samples before it was followed` under
`Recorded messages:` (the recording is not failed for it, as a node that starts during a
recording cannot be avoided); `echo` and `hz` print one such line on standard error; `--sync`
fails the recording on them. A stream is followed after the listing that shows it, and the listing waits for the
spacing: at 100 Hz the recorder counted 17 to 73 samples, and the recording began at the
counter it named, and at 1 kHz 135 to 599 (`record_late_streams.rs`, runs on a loaded
machine; before the count existed the same late streams were recorded from 22 to 55 and from
1397 with no word).

**Not followers.** `hypha topic list` lists the streams that exist when it runs, once.
`hypha intercept` relays one named input and subscribes to the streams that exist when it
starts; it relays one type, and a change of the type is the configuration's to make. Neither
follows a stream that joins later.

---

## Recording and replay

### `hypha record` — capture to MCAP

```text
hypha record [--topics <pattern>...]
           [--output <file.mcap>] [--compression <lz4|zstd|none>]
           [--sync] [--nodes <name>...]
```

Follows every topic that matches a `--topics` name or pattern (`*` is one
segment, a trailing `**` the rest; default `**`) and writes an MCAP file with
chunk compression `lz4` (default), `zstd` or `none`. Each topic's
schema is recorded alongside the data, so a recording is self-describing and
can be decoded later without the originating node.

Each followed topic is subscribed with depth 0, which the transport reads as the
largest depth the topic's service policy admits: 32 under the `default` policy. A
recorder without `--sync` is an observer. When a followed topic overruns that queue,
its oldest samples are lost. The subscriber counts the sequence numbers it missed, so
a loss is not a sample the recorder dropped. Without `--sync` the recorder reads each
topic's cumulative count on standard error once every 10 seconds and again at the end,
prints `hypha record: <topic>: <n> samples lost: ...` when the count grew (the line names
the remedy: a larger `maxDepth` in the topic's QoS policy, or `--sync`), lists each topic
that lost samples under `Recorded messages:` as `  <topic> lost <n> samples`, and exits
1, because the recording is incomplete. A sample that cannot be recorded is dropped
with one warning per topic; that is not a loss and does not change the exit code.
Recording a fast topic completely is a job for a replay: run `hypha record --sync`
beside `hypha play --clock`.

`--sync` makes the recorder a participant of a `/clock` replay
(`replay-flow-control.md`, section 6.3). Its reports carry its node name, so a
`--sync` recorder joins as `hypha_record` (the name `hypha play --nodes` takes), and one runs in a domain at a time. It holds each chunk until the chunk is
written, so the recording holds every output of the replay. It stops, with an
error, when a followed topic loses a sample in one chunk, or when a sample cannot
be recorded. The recording is then finished, and the player stops with the
recorder's name. Lower `hypha play --chunk` when a topic bursts more than 32
samples within a chunk. `--nodes <name>...` makes the first chunk wait until each
named node has reported, so none is missed; it needs `--sync`. Without it, the
first chunk may be acknowledged before a node that has not reported yet is heard.
The recorder announces itself to the player, and with `--topics` it answers no chunk
before every topic named without a wildcard has a publisher, so any start order of player, recorder and
node records every output.

MCAP `log_time` and `publish_time` are both the frame's framework send stamp, in
nanoseconds since 1970-01-01: the time its publisher gave it. On host time that is
the sender's UTC (a cycle output carries its step's time, the wall time read when
the step started, not the stamp of the trigger sample that caused it); in a replay on simulated time it is the replayed timeline. A vehicle
recording replayed in simulation with its inputs, and recorded, therefore keeps the
vehicle's times: the inputs carry them exactly, and a cycle output carries its
simulated step time, which differs from the vehicle's by the vehicle step's wake-up
delay. A frame with no framework stamp is dropped and reported. A recording of a
looping replay holds each pass with the same times again, in the order they were
received; replay it from one pass with `--start-utc`/`--end-utc`. Payload sensor
timestamps remain unchanged.

**One channel for each stream.** The recorder follows streams, not names: a topic with two
types, or two versions of one type, is two MCAP channels of one topic, each with its own
schema (the definition of its stream) and its `md5sum` metadata, and a stream that joins a
followed name after the recording began, a second version in a migration, is recorded from
the moment the recorder followed it, and the recording says how many samples it sent before
("What a late stream sent before it was followed", above). It says `hypha record: following
<n> more stream(s), <m> in all` on standard error when one joins. A recording of one type on a topic is the file the recorder
always wrote, byte for byte (MCAP library 0.25; with more than one channel the library
writes the records of its summary in hash-map order, so two runs differ in the order of
records, not in content). The counts under `Recorded messages:` and a loss line are per
stream: a topic of several streams is shown as `<topic> [<type> (<md5 start>)]`, one of one
as `<topic>`. Under `--sync` a second stream is no failure, since each stream is a channel of
its own: the recorder follows a stream that came before a chunk's data before it answers
the chunk. A stream that appears during a chunk is followed when the chunk is answered, so
what it sent first is not in the recording: `--sync` fails, naming the stream and the count.
A stream that exists and refuses the recorder's subscription (other tools hold its observer
slots) fails a `--sync` recording at once, naming the stream and the limit; an observer says
so on standard error and records its other streams. What stays by name: `--topics <name>` and `--sync`'s wait for
"every `--topics` entry has a publisher" are satisfied by a stream of any schema. A sample
that cannot be recorded (its schema unresolved, no framework stamp) is dropped and reported
once for its stream, and fails a `--sync` recording; the recorder does not silently write a
stream without a definition.

### `hypha play` — replay a recording

```text
hypha play [--input <file.mcap>] [--rate <factor|max>] [--loop] [--clock]
           [--nodes <name>...] [--settle <s>] [--chunk <ms>] [--timeout <s>]
           [--topics <t>...] [--start-utc <timestamp>] [--end-utc <timestamp>]
```

Replays in recorded timestamp order, paced against the recording's own
timeline scaled by `--rate`: a factor, or `max` for no pacing. Every replayed
topic notifies, as every publisher does, so a callback subscriber wakes per sample. A topic
that the player opens first is created with the `default` policy (`maxDepth` 32): a
node that starts after the player and declares another policy for that topic is
refused with `QosMismatch`. Start the nodes first, or map the type to `default`
(cpp-api.md §7). The queue depth is independent of the node's arrivals-per-step
processing budget.
A topic whose service a publisher opened keeps that service's queue, which is
8 by default. A subscriber cannot create the service: it waits for the topic to be
published. The player then publishes it with 8 and says so.

A recording that holds two channels of one topic, two types or two versions, is replayed
on two streams: the player makes one publisher for each topic and MD5 (two channels of
the same MD5 share one), so a typed subscriber receives exactly the frames of its version,
in order. Under `--clock` the chunk and queue rules count each stream on its own: a chunk
is split before the message that would overflow the smallest subscriber queue of *its*
stream (`hypha play: chunks shortened on '<topic>' [<type> (<md5 start>)]`), and the count of
published messages is per stream, each stream under its topic and its type and MD5 start
when the topic has several. What stays by name, on the cautious side: `--topics` selects a
name, so every stream of it; the recording's own `/clock` is skipped under `--clock`
whatever its schema; `/clock` already published by any process, and a replayed name that
another process already publishes in any schema, are refused or reported as before.

A replayed topic that cannot be published refuses the start: a topic whose type has no
resolvable MD5, or whose publisher is refused. The player lists each one on standard
output as `'<topic>' cannot be published: <reason>`, publishes nothing and exits 1.
A read error in the recording stops the replay where it occurs and exits 1. The error
names the recording and, when the reader gives one, the file offset of the chunk data
it was reading; a recording that cannot be read in full does not open. A replayed
topic that another process in the domain already publishes is reported once on
standard error, `hypha play: <topic> already has a live publisher; replayed data will
mix with it`, and the replay goes on. The check runs once, at the start.

`--clock` also publishes `/clock`, which lets nodes started with
`HYPHA_USE_SIM_TIME=1` follow the recording's time rather than the wall clock.
Only one player may run in a domain: it refuses to start while another process
publishes `/clock`, and a ROS clock bridged onto the bus counts as one.
With `--clock` the player sends the recording in chunks. A chunk is `--chunk <ms>`
of data time (default 100). The player publishes the chunk's messages, then
`/clock` for the chunk's end, and waits until every participant has reported on
`/clock_progress` that it has run the deadlines of that chunk. A participant is a
node that publishes progress, and a `hypha record --sync` recorder.
`--settle <s>` (default 2) starts the replay once no new node has reported for
that long. `--nodes <name>...` adds required nodes: the start also waits for each of
them and fails after 30 s if one is missing. Nodes that report without being named
take part too. A wait longer than 1 s prints the nodes it waits for. A participant
whose clock does not rise for `--timeout <s>` (default 5), quiet or not, stops the
replay with exit 1, and the error names every awaited node and what it did
(`replay-flow-control.md` §6.1). `--rate`
paces each message on the wall clock; `max` waits only for the participants. A report
wakes the player's wait at once (no fixed poll); an unanswered `/clock` is sent again
every 200 ms.

With `--clock`, `--loop` and the restart key start a pass again from the start of
the window, which sends `/clock` back. That restarts the participants: no node
rewinds in place. The player sends the first `/clock` value once, waits until every
participant has left the bus (each C++ node built on
`hypha::runNode` exits 75 and its `hypha launch` supervisor starts it again), opens its publishers again, and waits for the
same participants to rejoin before the pass sends data. A `hypha record --sync`
recorder stays through it. A participant that is still on the bus or has not
rejoined after 30 s stops the replay; a node with its own `main`, or a Rust node,
does not restart itself (`replay-flow-control.md`, section 4.4).

When the replay ends with `--clock`, the player prints one line with the
effective rate, the data time over the wall time, and the participant that set
it, for example `effective rate 0.98x: 12.300 s of data in 12.550 s; set by
'<node>' (last to answer ...)`. If `--rate` set it, the line says so.

Every message of the recording is published: the replay never skips data to keep
its schedule. A replay that cannot keep its schedule runs late, and the effective-rate
line says what set the rate (`replay-flow-control.md`, section 6.2).
The player serves its executor while it waits, so the MCAP schema records, which
are republished as hypha schema RPC servers, answer during the whole replay. Late
subscribers and the ROS 1 bridge can resolve each replayed frame. Replay only a
complete recording: the player maps the file into memory, so a recording that
`hypha record` is still writing must not be replayed, and nothing may change or
truncate a recording while it is replayed.

To investigate a customer report at 12:00 UTC, select a surrounding window:

```sh
hypha play --input incident.mcap --clock \
  --start-utc 2026-10-08T11:59:30Z --end-utc 2026-10-08T12:00:30Z
```

Start is inclusive and end is exclusive. Timestamps need `Z` or an explicit
timezone offset; offsets are normalized to UTC. The reader uses the MCAP
index to select the window. Recordings can also be searched by UTC in an MCAP
viewer such as Foxglove Studio.

For a system rerun, start the nodes with `HYPHA_USE_SIM_TIME=1` in their environment.
A replay has two modes and one rule: use the lockstep, the default; if the system cannot
run in it, set `HYPHA_REPLAY_LOCKSTEP=0` for the whole launch.

`HYPHA_REPLAY_LOCKSTEP=0` (or `false`), set for the player and every node of the
launch, turns the lockstep off: an open-loop playback like `ros2 bag play`. With
`--clock` the player prints `hypha play: open loop (HYPHA_REPLAY_LOCKSTEP=0): /clock
and the data follow --rate, and no node is waited for`, publishes each chunk's data on
the `--rate` schedule and `/clock` once at each boundary, and reads no
`/clock_progress`: no start barrier, no acknowledgement, no resend, no chunk split for a
subscriber queue, and no wait for a restart after `--loop`. `--rate max` is refused, as
nothing would pace the replay, and so are `--nodes`, `--settle` and `--timeout`. The
nodes follow `/clock` and warn of a lost sample instead of failing; a node still exits
75 when `/clock` goes back. The result depends on the host and its load, as a ROS
playback's does (`replay-flow-control.md`, section 4.6). Any other value than `1`,
`true`, `0` or `false` refuses the start. With `--clock`,
playback initializes `/clock` to the selected UTC start, advances it at each chunk
boundary, and sets each message's UTC before publishing its payload. Recorded `/clock` payloads are omitted so they cannot replace that
timeline. Select enough data before the incident to initialize application
state and include the required input topics. Replay preserves the payload bytes.

Every frame carries its recorded time as its framework send stamp
(`framework_sent_nanos`), including `/clock`. A subscriber reads it from the
frame, so it sees the recording's time, not the host's. `/clock = T` goes out only
after every message stamped at or before `T`, and the player waits until every
participant has run the deadlines up to `T`; the rule and its one exception, the
first instant of a pass, are in `replay-flow-control.md` §4.3.

A cycle node that runs on simulated time replays faithfully at any `--rate` the
host keeps up with. Its steps run on the recording's deadlines, with the samples
recorded before each deadline, and its data ages are measured in recorded time.
A step that loses a sample on a queued input (a transport overflow or a history
eviction) fails with `SequenceGap` and logs `replay not faithful`. A node drains
its inputs on every `/clock`, so a queue holds one chunk, which the player splits
to fit it. A queued input needs a `perStep` of at least the cycle's period times
the topic's rate; a `Latest` input on a topic the player alone publishes needs
nothing more (`replay-flow-control.md` §6.7). Another publisher on the topic is not
split into chunks, and one that keeps sending while the node drains its inputs can
leave samples queued after the node's report. The execution model (section 11.1)
describes the rules.

Observers, such as a recorder or the bridge, are not waited for. A message sent
before one connects is lost to it.

Interactive keys while playing: pause (`<Space>`), step one chunk (`<s>`), speed
up and slow down (`<+>` and `<->`), restart (`<r>`), quit (`<q>`). The keys and the
progress line need a terminal on stdin and stdout that the player may read: it must be
in the terminal's foreground process group. As a shell's background job or a
`hypha launch` unit the player takes no keys and prints plain lines.

### `hypha convert` — a ROS 2 bag into a recording

Turns a ROS 2 bag in rosbag2's SQLite storage (`.db3`) into an MCAP recording that
`hypha play` replays, without ROS (#46). Uncompress a `.db3.zstd` bag with `zstd -d`
first.

```bash
hypha convert run.db3 -o run.mcap --msg-path <dir>    # every topic it has a definition for
hypha convert run.db3 -o run.mcap --topic /imu --topic /lidar/points --xyzirc /lidar/points
```

- **Serialization.** Each topic's CDR messages become ROS 1 serialization through the
  ROS 1 definition of its type, which the recording carries with its MD5, as
  `hypha record` writes them. Bags recorded before rosbag2 embedded definitions hold
  none, so the definitions come from the message paths: each `--msg-path`, in order,
  then the SDK's `share/hypha/msg`. A path holds `<package>/<Name>.msg` or
  `<package>/msg/<Name>.msg`.
- **ROS 2 types.** `std_msgs/Header` has no `seq` in ROS 2: the converter writes the
  message's index on its topic. `builtin_interfaces/Time` becomes `time`, and a negative
  second fails the conversion, as ROS 1's `time` is unsigned; `Duration` becomes
  `duration`.
- **Times.** A message's log and publish time is the bag's receive time.
- **What is left out.** A topic whose type has no definition, or that is not CDR, is left
  out and listed with its reason. One named with `--topic` fails the conversion instead.
  A message that does not decode fails it, naming the topic and the message.
- **Point layouts.** `--xyzirc <topic>` rewrites an XYZI `sensor_msgs/PointCloud2`
  (four `float32`) into the XYZIRC that Autoware 2.1.0 reads: `intensity` and
  `return_type` as `uint8`, `channel` as `uint16`, still 16 bytes a point. Each intensity
  is rounded and held to 0 to 255, return type and channel are 0, and `row_step` is
  recomputed. The tool reports the highest intensity it saw. When every intensity rounds
  to 0, it says the recording holds no intensity on that scale.

---

## Running a system

### `hypha launch` — run a set of nodes in one tmux session

```text
hypha launch up     <file.toml> [--session <name>] [--attach]
hypha launch attach <file.toml> [--session <name>]
hypha launch down   <file.toml> [--session <name>]
```

`up` starts each unit in its own window of a detached tmux session. The
output of every node is in one place: an operator connects over SSH, runs
`hypha launch attach fleet.toml`, and moves between windows with `Ctrl-b n`
and `Ctrl-b p`, or `Ctrl-b w` for a list. `Ctrl-b d` detaches and leaves
the fleet running. `down` stops every unit and removes the session.

The session has its own status line: `hypha` and the session name on the left, the
host and time on the right, and one entry per unit, in the Mycelium palette. A unit
whose process ended for good shows in the error colour with `✗`. These are options of
the session and its windows only; the tmux configuration of the user stays as it is for
every other session.

The session name is the file name without its extension (`fleet.toml`
gives `fleet`). `--session` gives another name. Names hold only ASCII
letters, digits, `_` and `-`. `up` refuses to start a session that already
exists. `attach` and `down` use the file name only; they do not read the
file.

Start the fleet at boot from the init script of the target:

```sh
hypha launch up /etc/hypha/fleet.toml
```

tmux 3.0 or newer is required. There is no mode without tmux.

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

A unit may not use the name `hypha_launch_setup`: `up` uses that window while
it builds the session.

Every unit starts with the environment of the `up` command, also when a tmux
server is already running, plus its own `[units.env]`. `TMUX` and `TMUX_PANE`
are left out, and a variable that is not UTF-8 fails `up`.

#### What runs in a window

Each window runs `hypha launch unit <file> <name>`, a supervisor for one
unit. Startup holds each unit at a tmux barrier until its window is configured
with `remain-on-exit`, then executes the supervisor. This avoids replacing a
live pane's PTY on macOS and keeps even immediate failures visible. The node writes directly to the window, so its scrollback holds all
runs. On each exit the supervisor prints one line, for example
`[hypha launch] 'planner' killed by SIGSEGV; restarting in 1.0 s (attempt 3)`.
A unit that ends for good keeps its window, and tmux shows its exit status.
A unit that cannot start reports exit code 127 and follows its restart
policy.

The delay before a restart starts at `restartDelaySeconds` and doubles with
each restart up to 30 s; a first delay above 30 s stays as it is. A run of at
least 60 s (host monotonic time) resets the delay and the count. After 10
restarts with no 60 s run the supervisor prints
`[hypha launch] '<name>' gave up after 10 restarts with no 60 s run` and exits
with the node's last exit code. A node that keeps dying after 59 s is therefore
given up too. With `restartDelaySeconds = 0` the 10 restarts happen at once.

The node's exit code steers the restart before the policy does:

| Exit | Meaning | `hypha launch` |
|---|---|---|
| 0 | clean stop | restarted only under `always`, counted |
| 78 | configuration refused at start | never restarted, under any policy; `hypha launch unit` exits 78 |
| 75 | simulated clock went back | restarted at once under every policy, `no` included; not counted, no delay. The new run gets `HYPHA_REJOIN_AFTER_NANOS`, the supervisor's steady time when it saw the exit |
| 70, other, signal | runtime failure | restarted under `on-failure` and `always`, counted |

A systemd unit that runs a node directly gets the same rules with
`RestartPreventExitStatus=78` and `RestartForceExitStatus=75`.

Each node runs in its own process group, so `Ctrl-c` in a window reaches only
the supervisor, which stops that unit without a restart. SIGTERM, SIGINT and
SIGHUP to the supervisor send one SIGTERM to the node's process group (the node
and every child it started), and SIGKILL to the group after 5 s. `down` sends
SIGTERM to every supervisor, waits up to 10 s, sends SIGKILL to a supervisor
still alive, and then removes the session; the supervisor itself stops its
node's group at 5 s.

On Linux a node gets SIGTERM when its supervisor dies, even by SIGKILL (for
example from the OOM killer); its own children get nothing then, and a node
that ignores SIGTERM survives. The supervisor watches the node through a pidfd
and reaps it only after its last signal, so a signal never reaches a reused
process id. On macOS neither exists: a supervisor killed by SIGKILL leaves its
node running.

#### Limits

- **The fleet depends on the tmux server.** If the tmux server stops, every
  window gets SIGHUP and every node stops. `tmux kill-session` has the same
  effect, without the ordered stop that `down` gives.
- **Log output can block a node.** A node writes to a terminal, and one tmux
  thread reads all windows. A node that logs faster than tmux reads blocks
  in `write()`. An attached client on a slow link does not cause this. Do
  not log at a high rate on a hot path.
- **C and C++ stdio is line-buffered on a terminal**, so a `printf`-heavy
  node makes more write calls than with a pipe or a file.
- **Scrollback uses memory for each window.** Set `historyLimit` for the RAM
  of the target.
- **A node that calls `setsid` or `setpgid` leaves its process group**, and
  the supervisor's group signals do not reach it.
- **A node does not own the terminal.** It runs in a background process group of
  its window, so it gets no keys: a read from the terminal stops it with SIGTTIN,
  or fails with `EIO` when the node ignores or blocks that signal. Output is not
  affected. `hypha play` as a unit sees this, takes no keys and prints plain lines.

#### Measured cost

Measured on macOS 26.6, Apple M1 Pro (10 cores), tmux 3.4, debug build of
`hypha launch`, with 20 `fluid_particle` nodes (`--count 20 --seed 1`). CPU is
the CPU time used during a 15 s window, as a percentage of one core. The rate
is the last `hypha topic hz /fluid/p/0` report.

| Run | Particle CPU (sum) | `hypha launch unit` CPU / RSS (sum) | tmux server CPU / RSS | `/fluid/p/0` rate |
|---|---|---|---|---|
| Direct, no tmux | 15.1 % | — | — | 19.998 Hz |
| `hypha launch up` | 15.3 % | 0.0 % / 49.9 MB | 0.0 % / 3.9 MB | 20.003 Hz |
| `hypha launch up` + one unit running `yes` | 4.7 % | 0.0 % / 38.1 MB | 99.5 % / 23.5 MB | 20.013 Hz |

- `hypha launch up` compared with the direct run: particle CPU differs by
  0.2 percentage points and the rate by 0.005 Hz. The supervisors used no
  measurable CPU. The cost is 49.9 MB of supervisor memory and 3.9 MB for the
  tmux server.
- With a unit that writes log lines without limit, the tmux server used 99.5 %
  of one core and `yes` itself only 5.9 %: the writer spent most of its time
  blocked in `write()`. The particles do not log, and their rate stayed at
  20.0 Hz. The particle CPU was 4.7 %; this measurement does not show why it
  is lower than in the other runs.
- `fluid_particle` does not exit on SIGTERM. `hypha launch down` therefore took
  5 s, the supervisor grace period, and the supervisors stopped the particles
  with SIGKILL.

### `hypha param` — inspect a configuration file

```text
hypha param dump <file>                every scalar key=value pair, sorted
hypha param get <file> <key>           one value, or exit 1 if absent
hypha param validate <file>            prints "<file>: valid", or the refusal and exit 1
```

All three load the file as a node loads its parameter file: its `include` files,
`${NAME}` references and relative paths, and the refusal of a key that two files
set. A file that `validate` accepts is one a node can load; a refusal prints the
node's message after `hypha param: `. Whether the node reads every key is checked
only when it starts. The tool does not touch the bus or write the file. Edit the
file by hand, so its comments and key order stay as written.
There is no live parameter server — parameters are read at node start-up.

---

### `hypha clean` — remove the shared memory of domains nobody uses

```text
hypha clean --domain <prefix>   every domain whose name starts with <prefix>
hypha clean                     the domain of HYPHA_DOMAIN, the domain "none" when unset
```

A process that ends without its cleanup, after a crash or a kill, leaves its domain's
iceoryx2 resources behind, and a segment made by another hypha build or with other
limits fails the next open (`docs/engineering-traps.md`). `hypha clean` removes every
resource of a domain nobody uses: the shared-memory segments (`/dev/shm` on Linux, the
objects that `/tmp/iox2*.shm_state` names on macOS) and the services, node files,
sockets and schema files under `/tmp/iceoryx2`. It matches names inside the per-node
directories `nodes/<id>/`, which several domains share, and removes such a directory
only once it is empty. It kills nothing.

It keeps a domain while one of its nodes lives, as iceoryx2's node monitor reports it:
the lock a live process holds on its node's monitor file, which the kernel releases when
the process ends. A node of another user, or one whose files are inconsistent, also
keeps its domain. Under `--domain`, it also keeps a domain while its run lives: the
process whose id follows the prefix in the domain's name, so a test domain `t_<pid>_<n>`
stays between the tests of a running suite. A process that holds a resource without a
node of the domain is not seen. A node in the middle of its start is not yet listed, so
a clean of a domain that is starting can remove what that node just created.

It prints a line for each domain it removed (with its count of resources), each it
kept and why, and each resource it could not remove, then a summary line:

```text
hypha clean: removed domain t_4711_3 (21 resources)
hypha clean: kept domain t_5120_1: node sensor_driver of pid 5133 is alive
hypha clean: of the domains under 't_', removed 1, kept 1, failed 0
```

Exit codes: 0 when nothing is left that could be removed (it removed every domain it
covered, or there was nothing to remove), 2 when it kept a domain in use (it may have
removed others), 1 on a failure (a command line it refuses, a resource it could not
read or remove). The printed lines tell a removal from nothing to remove. A caller
decides what a kept domain means for it: `test.sh` and `examples/check.sh` accept 2,
since a kept test domain belongs to another run that still lives, and fail on 1.
`hypha clean` without `--domain` never touches another domain, even one whose name
starts with this one.

To wipe every domain of the host, and to kill the processes that still hold a resource,
the repository keeps `./clean-shm.sh [--force]`, which needs `lsof`.

## Testing communication

### `hypha intercept` — rewrite fields between a producer and its consumers

```text
hypha intercept [--name <backend>] [--config <path>]    defaults: default, ./intercept.toml
```

A headless man-in-the-middle relay. Each interception reads its `input` topic,
rewrites fields of each frame, and publishes the result on its `output` topic under
the input's type identity (`data_type`, `md5`, definition), keeping the producer's
framework send time. It needs no generated type: the schema is resolved from the
producer at run time. Point the consumers at `output`, or the producer at `input`,
to put the relay between them. The backend runs as node `hypha_intercept_<backend>`,
also when no `hypha top` is connected; its Intercept tab is a frontend for it.

The config file holds the desired state. The backend reloads it when it changes;
`hypha top` writes it atomically (temporary file, then rename). All values are text,
parsed against the field type when applied, so the file does not depend on a schema:

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

Paths are `a.b`, `arr[3].c` and `data[2]`. Integers take decimal, `0x` or `0b`, with
an optional `-`; `time` and `duration` take `sec.fraction`. A string override may
make the message longer: the output payload grows. No override changes an array
length, and byte arrays (`uint8[]`, `byte[]`, `char[]`) cannot be overridden. A
config that does not parse, or that holds an invalid name, a duplicate name, an
interception reading its own output, two interceptions writing one output, or an
override that does not parse as text, is not applied: the previous one stays, and
`config_error` says why.

The applied state is a latched `std_msgs/String` on
`/hypha/intercept/<backend>/state`, published on every change and at least every
500 ms. Its `data` is a TOML document:

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

Nothing is invented and nothing is hidden. Frames that arrive before the input's
schema resolves cannot be republished, so they are dropped and counted. A frame
whose payload does not match the schema is forwarded unchanged and counted in
`patch_failed`. An override whose path one message does not hold (`cells[5]` while
`cells` has three elements) fails that path only; the other overrides apply. When
the producer restarts with another schema, the interception stops forwarding
(`schema_changed`) rather than patch at stale offsets. Any later change of the
file rebuilds a stopped interception on the current schema (in `hypha top`, press
`a` twice). Inputs wake the relay
on each sample.

---

## Measuring the framework

### `hypha load` — latency, loss and memory under a defined load

```sh
hypha load examples/load/smoke.toml --report smoke.json [--recording smoke.mcap]
```

A workload file names worker processes, the topics each one publishes and subscribes
to, the run's times, the recorder and the pass limits. `hypha load` starts one worker
per `[[process]]` (this executable again, in a hidden worker mode), leads them through
the run, samples their memory, writes one JSON report and prints a short summary. Each
worker is an ordinary bus node, `load_<process>`, so the workload decides which
publishers and subscribers share a process. The run uses a domain of its own,
`load_<pid>` (`<HYPHA_DOMAIN>_ld<pid>` when the caller set one), so it cannot touch a
running stack; it removes that domain's shared memory at the end and names a domain it
had to keep. A coordinator killed with SIGKILL ends no child: on Linux each worker and
the recorder get SIGTERM from the parent-death signal, and a worker also stops when its
stdin closes, but on macOS the recorder keeps running. Stop it by the pid the
coordinator printed (`hypha load: the recorder runs as pid N`), then run
`hypha clean --domain load_`.

Exit codes: `0` every limit passed, `1` a limit failed, `70` the run itself failed: an
invalid command line or workload, a worker that died, a worker silent for more than
three sample intervals (at least 10 s), a start-up or end phase longer than 30 s, an
interrupt, or a recorder that died during the run, ended by a signal, exited with a code
other than 0 or 1, or left a recording that cannot be read. `hypha record` exits 1 when
it lost samples; with a readable recording that is no failed run, and the limit
`max_unrecorded` judges the loss. The report's `recorder.exit_code` says how it ended.
The report path is removed when the run starts, and the report is written beside it and
renamed into place, so the path holds this run's whole report or nothing. A failed run
still writes its report when it can, with `"outcome": "run_failed"` and the reason; an
invalid workload writes none. The summary is printed after the report is written; a
report that cannot be written prints `hypha load: RUN FAILED` and exits 70.

The workers use the Rust SDK. The runtime below the C ABI is the one a C++ node uses;
the thin C++ layer is not in the measured path.

#### Workload

| Key | Required | Meaning |
|---|---|---|
| `[run] duration_s`, `warmup_s`, `cooldown_s` | yes | The measuring window, the time before it that no statistic counts, and the time after it in which publishers go on sending so that a lost last frame of the window shows as a gap |
| `[run] sample_interval_s` | yes | The period of the time series and of the workers' heartbeat |
| `[run] bounded_memory` | yes | Runs the workers as a bounded deployment (`HYPHA_BOUNDED_MEMORY`) |
| `[recorder] enabled`, `compression` | no (off, `lz4`) | A real `hypha record` child that records every topic of the run |
| `[[process]] name` | yes | A worker, `[a-z][a-z0-9_]*`; `recorder` is the report's name of the recorder and is refused |
| `[[topic]] name`, `bytes`, `rate_hz`, `publisher`, `subscribers` | yes | One topic: its message size (at least 20 bytes, the probe header), its rate (messages per second, or `"max"` for back to back), its publishing process and its subscribers |
| `[[topic]] burst = { count, every_s }` | no (none) | `count` extra messages back to back every `every_s` seconds |
| subscriber `process` | yes | The process it runs in |
| subscriber `depth`, `work_us` | no (0, 0) | Its queue depth (0: the deepest the topic's service admits, 32 for the `default` policy that a load publisher creates) and the busy work per sample that makes a slow reader |
| subscriber `limits` | no (the run's) | Subscriber limits of its own, which replace those of `[limits]` |

An unknown key is refused, and so is a process with nothing to do, a topic with two
subscribers in one process, and a topic with more than three subscribers, the recorder
counted: a topic admits three subscribers that declare no policy, as the workers and the
recorder do. A process is refused when the full queues of its subscribers times their
`work_us` reach the heartbeat bound: one turn of its `WaitSet` would then hold its
heartbeat, and the message names both numbers. A slow reader shares the `WaitSet` of its
process, so it holds up every other callback there: give it a process of its own.
Each message is a `hypha_load/Probe` (`uint64 sent_nanos`, `uint64
sequence`, `uint8[] fill`), so a recording of the run decodes like any other.

#### Limits

Every key is optional, and an absent key is not checked. A limit whose value was not
measured fails.

| Key | Checked on | Fails when |
|---|---|---|
| `latency_p99_us` | each subscriber | the 99th percentile over the window exceeds it |
| `max_interval_latency_p99_us` | each subscriber | the 99th percentile of any one sample interval of the window exceeds it, taken from every interval as it ends |
| `max_loss` | each subscriber | the frames lost exceed it: the framework's missing sequences from the window's start to the stop (the cool-down included), or the window's messages not received when that is more. A message of the window still queued at the stop counts as lost |
| `loss_accounted` | each subscriber | the framework's missing-sequence count differs from the harness's own count of skipped probe sequences, or a probe came out of order or was no probe; it reports that difference plus those probes, against a limit of 0 |
| `max_refused_publishes` | each publisher | more publishes than this were refused over the run |
| `max_publish_call_us` | each publisher | the longest publish call of the window exceeds it |
| `max_missed_periods` | each periodic publisher | more of the window's deadlines than this were missed; a `"max"` topic has no periods and no result |
| `max_unrecorded` | each topic | more messages than this, of all the run sent, are missing from the recording; without the recorder it is not measured and fails |
| `max_rss_growth_bytes` | each process | its largest resident memory from the window's start to the end of publishing exceeds the one at the window's start by more |
| `max_shm_growth_bytes` | the domain | its written shared memory grows by more over the same span |

#### How it measures

- **Latency** is send call to callback entry. The publisher reads `CLOCK_MONOTONIC`
  just before its publish call and writes it into the probe; the subscriber's
  `WaitSet` callback reads the same clock first. Every process of a host shares that
  clock, and it never steps. A histogram of 3,776 fixed log-linear buckets (64 per
  power of two, at most 1/64 or about 1.6 % wide) keeps it; a quantile is the upper
  bound of its bucket, clamped to the exact maximum, so it never reads low and at most
  about 1.6 % high. The publish call's own duration has a histogram per publisher.
- **Loss** is the subscriber's missing-sequence count (`RawSubscriber::counters`), read
  at the window's start and at the stop. The harness counts the gaps of the probe's own
  sequence beside it, and the messages sent and received in the window, by send stamp.
  A refused publish moves the probe's sequence as the framework's: the framework takes a
  sequence number for a publish its pool or its send refused, so it is a gap for both.
- **Missed periods**: a publisher keeps absolute deadlines `start + k * period`. When
  it gets to send, it sends one message for the latest deadline that passed and counts
  the earlier ones as missed, the way a timer does; it does not catch up. A deadline
  counts when it lies in the window, whenever the publisher noticed it, and a deadline of
  the window that was never reached because publishing ended counts too. A burst adds
  messages and has no deadlines of its own; the time it takes can make the next deadline
  late. A sleep that wakes a period late (a loaded host, macOS timer coalescing at 1 kHz)
  is a missed period of the host, not of the harness.
- **Memory**: the coordinator reads each process every sample interval. Linux gives
  `VmRSS` and `RssShmem` from `/proc/<pid>/status`, and the domain's written shared
  memory as the allocated blocks of its `/dev/shm` objects. macOS gives the resident
  size from `ps` only; the report marks the shared-memory values `null` and says why
  under `not_measured`.
- **The report** holds the host (name, OS, kernel, CPUs, build profile), the parsed
  workload and its MD5, the domain, each topic's publisher and subscriber results, each
  process's memory, the recorder's counts, a time series and every checked limit. The
  series keeps at most 512 points: when it is full, neighbouring points merge, keeping
  the larger values. Neither the harness nor the report grows with the run's length.

A latency figure is a property of the host it was taken on: the report says which one,
and the test suite asserts no latency or speed (#64).

#### The rated load

`examples/load/rated.toml` is the rated load of NFR-04 (46 topics, 72.5 MB/s, 9,960
messages/s, MB = 10^6 bytes) with the limits of NFR-03 (p99 below 500 µs, send call to
callback entry) and NFR-04 (zero loss for readers that keep pace, every loss
accounted, no refused publish), and no missed period, so that the run offered the load
it names. The requirement gives only the totals; the per-topic split of the file is
constructed, and `load_workload.rs` recomputes the totals from it.

```sh
./build.sh
scripts/rated-load.sh [report.json]    # default target/rated-load.json
```

The script runs the release build `target/release/hypha` and does not build it; it
forwards the exit status of `hypha load`. `scripts/qualify.sh` runs it too, after the
C++ tests, as the measurement `rated_load` of the qualification report: its limits
failing turns nothing red, the run failing (exit 70, or no report) does, and no
requirement takes it as evidence.

A run counts for NFR-03 and NFR-04 only on the reference platform, which is not
defined yet ([sil2-readiness.md](sil2-readiness.md) G-10, G-11). Any other host is a
development host, and its report is a measurement of that host. Read it so:

- `host`: the host name, `os` and `os_release`, `arch` and `cpus` say which machine
  was measured; another load on it (a running stack, a build) is not recorded and
  shows as latency and missed periods.
- `host.build`: `release` or `debug`. A debug build measures unoptimised code; its
  latency says nothing about a release.
- `not_measured`: the quantities this host could not measure (on macOS the shared
  memory), each with its reason.
- A publisher's `missed_periods` above 0 means the rated load was not offered in
  full: the latency and loss of that run belong to a lower load.

#### The overload run

`examples/load/overload.toml` is a sustained overload for NFR-05: a topic sent back to
back to a slow reader, a 1 MiB topic to a slower one, bursts into a short queue, and two
control topics at 100 Hz between two processes that share nothing with the flood, for a
10 s warm-up and a 60 s window. Its limits are counted or bounded facts, not speeds:

- `max_refused_publishes = 0`: a slow reader makes the publisher discard that reader's
  oldest sample and never empties its pool, so no publish is refused. A publish call
  cannot block; its duration is a speed and is not limited.
- `loss_accounted` on every subscriber, the flooded ones included, and `max_loss = 0`
  on the control readers, which keep pace (a queue of 32 at 100 Hz loses a sample only
  when its reader is not scheduled for 320 ms).
- `max_rss_growth_bytes` per process and `max_shm_growth_bytes` for the domain, both
  1 MiB: memory after the warm-up stays flat. Every publisher is bounded, and
  `bounded_memory = true` runs the workers as a bounded deployment, the condition of
  NFR-05. The warm-up is long enough for every pool slot of every topic to have been
  written once (the file's header has the arithmetic); the shared-memory limit is
  unverified on Linux.

`examples/load/isolation-latency.toml` is the same workload with a latency limit on the
control topics (p99 below 500 µs); `load_workload.rs` checks that nothing else differs.
It is a measurement of the host, like the rated load.

```sh
./build.sh
scripts/overload.sh overload [report.json]            # default target/overload.json
scripts/overload.sh isolation-latency [report.json]
```

`scripts/qualify.sh` runs both after the rated load, as the measurements `overload` and
`overload_latency`: their limits failing turns nothing red, the run failing does. The
overload run is no evidence of NFR-05 until a Linux run confirms its limits; then it
becomes the reference `run:overload`.

Only Linux measures the written shared memory. On macOS that value is `null`, the
`not_measured` list names it, and `max_shm_growth_bytes` fails as not measured: there
the run exits 1 by design. Read the counted limits from the report's `limits` list
instead: every entry other than `max_shm_growth_bytes` with `pass: true`. On macOS the
resident memory includes shared pages, so `max_rss_growth_bytes` there also holds the
shared memory a process touched.

---

## Bridging

### `hypha_ros_bridge` — connect ROS 1 and hypha topics

Bridges ROS 1 Noetic topics and same-name hypha topics in one process, or the
routes of an explicit manifest. It carries message bodies as opaque bytes.
The transport age of a bridged ROS 1 sample starts at the bridge, because
ROS 1 sends no send time. See
[`apps/hypha_ros_bridge/README.md`](../apps/hypha_ros_bridge/README.md) for
options, the manifest format and the limits.

### `hypha foxglove` — view the bus in Foxglove Studio

Serves the bus over a WebSocket in the Foxglove protocol, so topics appear as
channels in Foxglove Studio. Run it, then connect Studio to the address it
prints.

It replays each channel's last value to a client that subscribes between
publishes, so a slow topic is not invisible until its next message. For
`/tf_static` it keeps one message per set of (parent, child) frame pairs, so a
late client receives every broadcaster's transforms and a re-stamped message
replaces its predecessor.

The bridge is a display: it drops data rather than fall behind. It reads each
topic through a queue of 8 samples, so a fast topic is sampled, not carried
whole. `--topics` takes names or patterns, as `hypha record` does, and is also
the subscription set (with `/clock`), so the bridge reads only what it shows. A sample that waits longer than `--max-latency`
(500 ms by default) for a slow client is dropped, and a client whose socket is
full is not disconnected. Time frames from `/clock` go out at most every
20 ms. For a replay at a high `--rate`, add `--max-rate <Hz>`: it limits each
topic in wall time, whatever the replay rate.

