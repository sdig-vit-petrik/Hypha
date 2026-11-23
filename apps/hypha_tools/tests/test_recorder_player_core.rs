//! Tests for pure MCAP recording and replay core functionality across all compression modes.

#![allow(clippy::unwrap_used)]
#![allow(clippy::indexing_slicing)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]

use hypha_tools::player::Player;
use hypha_tools::recorder::{McapCompression, RecordSample, Recorder};
use hypha_tools::utc_time::UtcRange;
use std::fs;
use std::path::PathBuf;

fn temp_mcap_path(name: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!("test_mcap_{name}_{}.{ext}", std::process::id()))
}

#[test]
fn roundtrips_channels_schemas_md5_and_bytes() {
    let path = temp_mcap_path("roundtrip", "mcap");
    let odom_def = "float64 data\n".to_string();
    let odom_md5 = "fdb28210bfa9d7c91146260178d9a584".to_string();
    let odom_bytes_a = vec![0, 0, 0, 0, 0, 0, 0x45, 0x40];
    let odom_bytes_b = vec![0, 0, 0, 0, 0, 0, 0x40, 0x40];

    let gripper_def = "uint8 STATE_LEFT=0\nuint8 STATE_RIGHT=1\nuint8 state\n".to_string();
    let gripper_md5 = "d9d213e8c146065ec958333c617eb1a6".to_string();
    let gripper_bytes = vec![1u8];

    {
        let mut rec = Recorder::new(&path);
        assert!(rec.is_open(), "error: {}", rec.open_error());

        let sample_a = RecordSample {
            topic: "/odom_speed",
            data_type: "std_msgs/Float64",
            md5: &odom_md5,
            definition: &odom_def,
            bytes: &odom_bytes_a,
            log_time_nanos: 1_000_000,
            publish_time_nanos: 1_000_000,
        };
        assert!(rec.record(&sample_a));

        let sample_b = RecordSample {
            topic: "/odom_speed",
            data_type: "std_msgs/Float64",
            md5: &odom_md5,
            definition: &odom_def,
            bytes: &odom_bytes_b,
            log_time_nanos: 1_500_000,
            publish_time_nanos: 1_500_000,
        };
        assert!(rec.record(&sample_b));

        let sample_sw = RecordSample {
            topic: "/gripper_report",
            data_type: "custom_msgs/GripperReport",
            md5: &gripper_md5,
            definition: &gripper_def,
            bytes: &gripper_bytes,
            log_time_nanos: 1_200_000,
            publish_time_nanos: 1_200_000,
        };
        assert!(rec.record(&sample_sw));

        let counts = rec.counters();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0], ("/odom_speed".to_string(), 2));
        assert_eq!(counts[1], ("/gripper_report".to_string(), 1));

        assert_eq!(rec.finish(), Ok(()));
    }

    {
        let player = Player::new(&path);
        assert!(player.is_open(), "error: {}", player.open_error());
        assert_eq!(player.channels().len(), 2);

        let mut items = Vec::new();
        let delivered = player
            .for_each_item(&[], UtcRange::default(), |item| {
                items.push((item.topic.to_owned(), item.bytes.to_vec(), item.delay_nanos));
                true
            })
            .unwrap();

        assert_eq!(delivered, 3);
        assert_eq!(items.len(), 3);

        // Delivered in log_time order: 1_000_000, 1_200_000, 1_500_000
        assert_eq!(items[0].0, "/odom_speed");
        assert_eq!(items[0].1, odom_bytes_a);
        assert_eq!(items[0].2, 0);

        assert_eq!(items[1].0, "/gripper_report");
        assert_eq!(items[1].1, gripper_bytes);
        assert_eq!(items[1].2, 200_000);

        assert_eq!(items[2].0, "/odom_speed");
        assert_eq!(items[2].1, odom_bytes_b);
        assert_eq!(items[2].2, 300_000);
    }

    let _ = fs::remove_file(path);
}

#[test]
fn roundtrips_zstd_and_uncompressed() {
    let test_compressions =
        [(McapCompression::Zstd, "zstd.mcap"), (McapCompression::None, "none.mcap")];

    for (comp, ext) in test_compressions {
        let path = temp_mcap_path("comp_test", ext);
        {
            let mut rec = Recorder::with_compression(&path, comp);
            assert!(rec.is_open(), "failed open {comp:?}: {}", rec.open_error());

            let sample = RecordSample {
                topic: "/imu/data",
                data_type: "sensor_msgs/Imu",
                md5: "1234567890abcdef1234567890abcdef",
                definition: "Header header\n",
                bytes: &[1, 2, 3, 4, 5],
                log_time_nanos: 500_000,
                publish_time_nanos: 500_000,
            };
            assert!(rec.record(&sample));
            assert_eq!(rec.finish(), Ok(()));
        }

        {
            let player = Player::new(&path);
            assert!(player.is_open(), "failed player open {comp:?}: {}", player.open_error());
            let mut count = 0;
            player
                .for_each_item(&[], UtcRange::default(), |item| {
                    assert_eq!(item.topic, "/imu/data");
                    assert_eq!(item.bytes, &[1, 2, 3, 4, 5]);
                    count += 1;
                    true
                })
                .unwrap();
            assert_eq!(count, 1, "expected 1 item for {comp:?}");
        }

        let _ = fs::remove_file(path);
    }
}

#[test]
fn topics_filter_excludes_unlisted_channels() {
    let path = temp_mcap_path("filter", "mcap");
    {
        let mut rec = Recorder::new(&path);
        assert!(rec.is_open());

        let kept = RecordSample {
            topic: "/kept",
            data_type: "std_msgs/Bool",
            md5: "8b94c1b53db61fb6aed406028ad6332a",
            definition: "bool data\n",
            bytes: &[1],
            log_time_nanos: 10_000,
            publish_time_nanos: 10_000,
        };
        assert!(rec.record(&kept));

        let dropped = RecordSample {
            topic: "/dropped",
            data_type: "std_msgs/Bool",
            md5: "8b94c1b53db61fb6aed406028ad6332a",
            definition: "bool data\n",
            bytes: &[0],
            log_time_nanos: 10_100,
            publish_time_nanos: 10_100,
        };
        assert!(rec.record(&dropped));

        let kept_again = RecordSample {
            topic: "/kept",
            data_type: "std_msgs/Bool",
            md5: "8b94c1b53db61fb6aed406028ad6332a",
            definition: "bool data\n",
            bytes: &[1],
            log_time_nanos: 10_900,
            publish_time_nanos: 10_900,
        };
        assert!(rec.record(&kept_again));
        assert_eq!(rec.finish(), Ok(()));
    }

    {
        let player = Player::new(&path);
        assert!(player.is_open());

        let mut items = Vec::new();
        let delivered = player
            .for_each_item(&["/kept".to_string()], UtcRange::default(), |item| {
                items.push((item.topic.to_owned(), item.bytes.to_vec(), item.delay_nanos));
                true
            })
            .unwrap();

        assert_eq!(delivered, 2);
        assert_eq!(items[0].0, "/kept");
        assert_eq!(items[0].2, 0);
        assert_eq!(items[1].0, "/kept");
        assert_eq!(items[1].2, 900);
    }

    let _ = fs::remove_file(path);
}

/// The player lists the channels in the order the recording made them, whatever the
/// hash seed of the process: the summary holds them in a hash map.
#[test]
fn channels_are_listed_in_recording_order() {
    let path = temp_mcap_path("channel_order", "mcap");
    let definition = "float64 data\n".to_string();
    let md5 = "fdb28210bfa9d7c91146260178d9a584".to_string();
    let topics: Vec<String> = (0..12).map(|index| format!("/topic_{index:02}")).collect();
    {
        let mut rec = Recorder::new(&path);
        assert!(rec.is_open(), "error: {}", rec.open_error());
        for (index, topic) in topics.iter().enumerate() {
            let time = 1_000_000 * (index as u64 + 1);
            assert!(rec.record(&RecordSample {
                topic,
                data_type: "std_msgs/Float64",
                md5: &md5,
                definition: &definition,
                bytes: &[0; 8],
                log_time_nanos: time,
                publish_time_nanos: time,
            }));
        }
        assert_eq!(rec.finish(), Ok(()));
    }

    let player = Player::new(&path);
    assert!(player.is_open(), "error: {}", player.open_error());
    let listed: Vec<&str> =
        player.channels().iter().map(|channel| channel.topic.as_str()).collect();
    assert_eq!(listed, topics);

    let _ = fs::remove_file(path);
}
