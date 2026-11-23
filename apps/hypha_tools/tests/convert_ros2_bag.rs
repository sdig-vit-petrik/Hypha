//! `hypha convert`: a rosbag2 SQLite bag becomes a recording that the player opens, with
//! each message in ROS 1 serialization; what cannot be converted is left out or refused.
#![expect(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "a test whose setup fails is a failed test; its sizes are small and fixed"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hypha_node::sdk::message::DecodeOwned;
use hypha_node::sdk::msgs::sensor_msgs::{Imu, PointCloud2, PointField};
use hypha_tools::player::Player;

/// A CDR writer: little-endian plain CDR, each primitive aligned to its size from the end
/// of the encapsulation header.
struct Cdr(Vec<u8>);

impl Cdr {
    fn new() -> Self {
        Self(vec![0x00, 0x01, 0x00, 0x00])
    }
    fn align(&mut self, size: usize) {
        while !(self.0.len() - 4).is_multiple_of(size) {
            self.0.push(0);
        }
    }
    fn u8(&mut self, value: u8) -> &mut Self {
        self.0.push(value);
        self
    }
    fn u32(&mut self, value: u32) -> &mut Self {
        self.align(4);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }
    fn f32(&mut self, value: f32) -> &mut Self {
        self.align(4);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }
    fn f64(&mut self, value: f64) -> &mut Self {
        self.align(8);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }
    fn text(&mut self, value: &str) -> &mut Self {
        self.u32(u32::try_from(value.len() + 1).expect("a short string"));
        self.0.extend_from_slice(value.as_bytes());
        self.0.push(0);
        self
    }
    fn header(&mut self, sec: u32, nanosec: u32, frame: &str) -> &mut Self {
        self.u32(sec).u32(nanosec).text(frame)
    }
}

/// An IMU whose orientation `w` is `w`, and every other value 0.
fn imu(sec: u32, w: f64) -> Vec<u8> {
    let mut cdr = Cdr::new();
    cdr.header(sec, 0, "base_link").f64(0.0).f64(0.0).f64(0.0).f64(w);
    for _ in 0..(9 + 3 + 9 + 3 + 9) {
        cdr.f64(0.0);
    }
    cdr.0
}

/// An XYZI cloud of two points, the second at intensity 200.
fn xyzi_cloud(sec: u32) -> Vec<u8> {
    let mut cdr = Cdr::new();
    cdr.header(sec, 0, "base_link").u32(1).u32(2).u32(4);
    for (name, offset) in [("x", 0), ("y", 4), ("z", 8), ("intensity", 12)] {
        cdr.text(name).u32(offset).u8(PointField::FLOAT32).u32(1);
    }
    cdr.u8(0).u32(16).u32(5).u32(32);
    for value in [1.0, 2.0, 3.0, 0.0, 4.0, 5.0, 6.0, 200.0] {
        cdr.f32(value);
    }
    cdr.u8(1);
    cdr.0
}

/// A rosbag2 SQLite bag of `topics` (id, name, type) and `messages` (topic, time, data).
fn write_bag(path: &Path, topics: &[(i64, &str, &str)], messages: &[(i64, i64, Vec<u8>)]) {
    let bag = rusqlite::Connection::open(path).expect("a new bag");
    bag.execute_batch(
        "CREATE TABLE topics(id INTEGER PRIMARY KEY, name TEXT NOT NULL, type TEXT NOT NULL, \
         serialization_format TEXT NOT NULL, offered_qos_profiles TEXT NOT NULL); \
         CREATE TABLE messages(id INTEGER PRIMARY KEY, topic_id INTEGER NOT NULL, \
         timestamp INTEGER NOT NULL, data BLOB NOT NULL);",
    )
    .expect("the rosbag2 tables");
    for (id, name, ros2_type) in topics {
        bag.execute(
            "INSERT INTO topics VALUES (?1, ?2, ?3, 'cdr', '')",
            rusqlite::params![id, name, ros2_type],
        )
        .expect("a topic");
    }
    for (topic, time, data) in messages {
        bag.execute(
            "INSERT INTO messages (topic_id, timestamp, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![topic, time, data],
        )
        .expect("a message");
    }
}

fn work_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hypha_convert_{label}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a work directory");
    dir
}

fn msg_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/hypha_msgs/msg")
}

fn convert(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("convert")
        .args(arguments)
        .output()
        .expect("hypha convert runs")
}

/// A bag with an IMU at 1 s and 3 s, an XYZI cloud at 2 s, and a vendor type at 1.5 s.
fn sample_bag(dir: &Path) -> PathBuf {
    let bag = dir.join("sample.db3");
    write_bag(
        &bag,
        &[
            (1, "/imu", "sensor_msgs/msg/Imu"),
            (2, "/cloud", "sensor_msgs/msg/PointCloud2"),
            (3, "/vendor", "vendor_msgs/msg/Thing"),
        ],
        &[
            (1, 1_000_000_000, imu(1, 1.0)),
            (3, 1_500_000_000, vec![0, 1, 0, 0, 7, 0, 0, 0]),
            (2, 2_000_000_000, xyzi_cloud(2)),
            (1, 3_000_000_000, imu(3, 0.5)),
        ],
    );
    bag
}

/// Every message of the recording at `path`: topic, log time and payload, in file order.
fn messages(path: &Path) -> Vec<(String, u64, Vec<u8>)> {
    let bytes = std::fs::read(path).expect("the recording");
    mcap::MessageStream::new(&bytes)
        .expect("an MCAP file")
        .map(|message| {
            let message = message.expect("a readable message");
            (message.channel.topic.clone(), message.log_time, message.data.into_owned())
        })
        .collect()
}

#[test]
fn a_bag_becomes_a_recording_the_player_opens() {
    let dir = work_dir("recording");
    let bag = sample_bag(&dir);
    let output = dir.join("out.mcap");
    let path = msg_path();
    let result = convert(&[
        bag.to_str().expect("a path"),
        "-o",
        output.to_str().expect("a path"),
        "--msg-path",
        path.to_str().expect("a path"),
        "--xyzirc",
        "/cloud",
    ]);
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(result.status.success(), "{stdout}{}", String::from_utf8_lossy(&result.stderr));
    assert!(stdout.contains("left out /vendor (vendor_msgs/msg/Thing)"), "{stdout}");
    assert!(stdout.contains("highest intensity 200"), "{stdout}");

    let player = Player::new(&output);
    assert!(player.is_open(), "{}", player.open_error());
    assert_eq!(player.total_messages(), 3);
    let identities: Vec<(&str, &str, &str)> = player
        .channels()
        .iter()
        .map(|channel| (channel.topic.as_str(), channel.data_type.as_str(), channel.md5.as_str()))
        .collect();
    assert_eq!(
        identities,
        [
            ("/imu", "sensor_msgs/Imu", Imu::SCHEMA_HASH),
            ("/cloud", "sensor_msgs/PointCloud2", PointCloud2::SCHEMA_HASH),
        ]
    );
    assert!(
        player.channels().iter().all(|channel| channel.definition.contains("MSG: std_msgs/Header"))
    );

    let recorded = messages(&output);
    let order: Vec<(&str, u64)> =
        recorded.iter().map(|(topic, time, _)| (topic.as_str(), *time)).collect();
    assert_eq!(
        order,
        [("/imu", 1_000_000_000), ("/cloud", 2_000_000_000), ("/imu", 3_000_000_000)]
    );
    let imus: Vec<Imu> = recorded
        .iter()
        .filter(|(topic, _, _)| topic == "/imu")
        .map(|(_, _, data)| Imu::decode_from(data).expect("an IMU"))
        .collect();
    let seen: Vec<(u32, u32, f64)> =
        imus.iter().map(|imu| (imu.header.seq, imu.header.stamp.secs, imu.orientation.w)).collect();
    assert_eq!(seen, [(0, 1, 1.0), (1, 3, 0.5)], "seq is the index on the topic");
    let (_, _, cloud) = recorded.iter().find(|(topic, _, _)| topic == "/cloud").expect("the cloud");
    let cloud = PointCloud2::decode_from(cloud).expect("a cloud");
    let names: Vec<&str> = cloud.fields.iter().map(|field| field.name.as_str()).collect();
    assert_eq!(names, ["x", "y", "z", "intensity", "return_type", "channel"]);
    assert_eq!((cloud.row_step, cloud.data.get(28).copied()), (32, Some(200)));
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn what_cannot_be_converted_is_refused_with_its_reason() {
    let dir = work_dir("refusals");
    let bag = sample_bag(&dir);
    let output = dir.join("out.mcap");
    let path = msg_path();
    let (bag, output, path) = (
        bag.to_str().expect("a path"),
        output.to_str().expect("a path"),
        path.to_str().expect("a path"),
    );
    let cases: [(&[&str], &str); 4] = [
        (&["--topic", "/vendor"], "no definition of vendor_msgs/Thing"),
        (&["--xyzirc", "/imu"], "--xyzirc needs sensor_msgs/PointCloud2"),
        (&["--topic", "/missing"], "the bag has no topic '/missing'"),
        (&["--topic", "/imu", "--xyzirc", "/cloud"], "is not among the topics to convert"),
    ];
    for (extra, reason) in cases {
        let mut arguments = vec![bag, "-o", output, "--msg-path", path];
        arguments.extend_from_slice(extra);
        let result = convert(&arguments);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(1), "{extra:?}: {stderr}");
        assert!(stderr.contains(reason), "{extra:?}: {stderr}");
    }
    let not_a_bag = dir.join("notes.txt");
    std::fs::write(&not_a_bag, "plain text").expect("a text file");
    let result = convert(&[not_a_bag.to_str().expect("a path"), "-o", output, "--msg-path", path]);
    assert_eq!(result.status.code(), Some(1));
    // The test binary has no installed SDK beside it, so it needs a message path.
    let result = convert(&[bag, "-o", output]);
    assert_eq!(result.status.code(), Some(2), "{}", String::from_utf8_lossy(&result.stderr));
    std::fs::remove_dir_all(&dir).expect("clean up");
}
