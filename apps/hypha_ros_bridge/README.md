# hypha_ros_bridge

`hypha_ros_bridge` is a temporary ROS 1 Noetic migration bridge. It connects
ROS 1 topics to same-name hypha topics in one process without configuration,
or uses an explicit manifest when a rename or an allow-list is required.

The bridge carries every message body as opaque bytes. It never decodes a
field and has no compiled-in message types, so any ROS 1 message type works,
including types private to the migration that were never known to this
repository. A route's `ros_type`, `ros_md5`, and `ros_definition` only label
the bytes. All three are optional for ROS 1 to hypha routes; when all are
present, the bridge validates that `ros_md5` matches the hash of
`ros_definition` before it opens any network connection. An hypha to ROS 1
route requires all three because ROS needs them in its connection header. In
automatic hypha to ROS 1 mode, the bridge resolves the compact frame schema
hash against the publisher's retained schema record and obtains these three
values from it. The ROS publisher for `/tf_static` is also created with
ROS 1 latching, so RViz can join after playback starts. The bridge never
parses message fields as a schema.

A ROS 1 message carries no send time on the wire. The bridge therefore
publishes each ROS 1 sample on hypha as a new frame, and hypha stamps its
transport send time then. The transport age of a bridged sample starts at
the bridge, not at the ROS 1 publisher. A hypha node that must judge the age
at the source reads the stamp in the message header.

With no `--config`, the bridge discovers both graphs continuously. It creates
a ROS 1 to hypha data pipe only for `ROS publisher + hypha subscriber`. For
hypha publishers, it advertises the matching ROS 1 topic after it resolves
the first complete retained schema, so ROS tools such as RViz can discover the
topic before they subscribe. It sends data only for `hypha publisher + ROS
subscriber` and removes the ROS advertisement when the hypha publisher
disappears.

Automatic ROS 1 to hypha routes carry the ROS publisher's real type, md5 and
expanded definition. A typed hypha subscriber accepts a frame only when its
schema hash matches the md5 of its generated type, so the bridge does not use
a placeholder. Before it routes a topic, the bridge asks the master for the
URI of each publisher node (`lookupNode`), asks each publisher for its TCPROS
endpoint (`requestTopic`), and reads the publisher's connection header on a
short connection of its own. It routes the topic only when every publisher
reports the same valid identity and that type agrees with the master. Else it
logs one error for each failure and tries again on the next graph poll.

The bridge looks up the publisher URIs again on every graph poll, every
500 ms. A new URI means that a publisher restarted, possibly with a changed
message definition. The bridge then drops samples on that topic until it has
read the identity again, and replaces its hypha publisher when the identity
changed. The ROS 1 client library does not report the connection that a
sample came from. Thus samples that arrive after a restart, but before the
next poll finds the new URI, keep the old identity.

Every topic that the bridge publishes on the hypha bus retains its schema
record, so readers in other processes, such as `hypha topic echo` and
`hypha record`, can resolve the definition of a bridged topic. A ROS md5 is
32 lower-case hexadecimal digits; a manifest's `ros_md5` is lower-cased when
it is read.

The explicit mode accepts one strict TOML manifest:

```toml
[[routes]]
hypha_topic = "/legacy/text"
ros_topic = "/legacy/text"
ros_type = "std_msgs/String"
ros_md5 = "992ce8a1687cec8c8bd883ec73ca41d1"
ros_definition = "string data\n"
direction = "bidirectional"
```

`ros_definition` is the expanded message definition text, as `gendeps --cat`
would print it (see <https://wiki.ros.org/roslib/gentools>): the message's
own field lines, followed by one `===...===`-delimited `MSG: pkg/Type`
section per nested message type it depends on. For a message with no nested
types, such as `std_msgs/String` above, it is just the field lines. Run
`rosmsg md5 <type>` and `rosmsg show -r <type>` against a real ROS 1
Noetic install to get a definition and md5 that are guaranteed to match; the
bridge refuses to start on a mismatch.

Start it with:

```sh
cargo run -p hypha_ros_bridge -- --config bridge.toml \
  --master-uri http://localhost:11311
```

Automatic mode:

```sh
cargo run -p hypha_ros_bridge -- --master-uri http://localhost:11311
```

## Manual ROS 1 end-to-end test

This test needs ROS 1 Noetic and is not part of CI.

1. Start `roscore` in one shell, or use the running master at
   `http://localhost:11311`.
2. Start the bridge without `--config`.
3. Start an hypha subscriber on a topic that the bag publishes, for example
   with `hypha topic echo /legacy/text`.
4. Confirm that the bridge discovers the matching ROS publisher and forwards
   the body to hypha.
5. Publish a value from ROS 1:

   ```sh
   rostopic pub -1 /legacy/text std_msgs/String '{data: hello}'
   ```

6. Publish the same body from an hypha node while `rostopic echo` is active.
7. Observe the ROS 1 topic with `rostopic echo /legacy/text`.
8. Send `SIGTERM` and confirm that the bridge exits without a worker error.

The test is successful when both directions carry the same message body and
the bridge exits cleanly.
