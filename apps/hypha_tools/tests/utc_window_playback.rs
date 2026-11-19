//! Customer UTC windows use the same boundaries in indexed and unindexed MCAP.
#![allow(clippy::expect_used)]

use hypha_tools::player::Player;
use hypha_tools::utc_time::{UtcRange, parse_utc_nanos};
use mcap::records::MessageHeader;
use mcap::{WriteOptions, Writer};
use std::collections::BTreeMap;
use std::fs::File;

#[test]
fn utc_window_filters_before_and_after_and_preserves_message_order() {
    let start = parse_utc_nanos("2026-10-08T12:00:00Z").expect("customer incident UTC");
    let end = parse_utc_nanos("2026-10-08T12:00:01Z").expect("window end");
    for indexed in [true, false] {
        let path =
            std::env::temp_dir().join(format!("utc_window_{}_{indexed}.mcap", std::process::id()));
        let file = File::create(&path).expect("recording file");
        let mut writer =
            Writer::with_options(file, WriteOptions::new().profile("ros1").use_chunks(indexed))
                .expect("writer");
        let schema = writer.add_schema("std_msgs/Bool", "ros1msg", b"bool data\n").expect("schema");
        let metadata = BTreeMap::from([(
            "md5sum".to_string(),
            "8b94c1b53db61fb6aed406028ad6332a".to_string(),
        )]);
        let channel = writer.add_channel(schema, "/incident", "ros1", &metadata).expect("channel");
        let before = start.checked_sub(1).expect("time before incident");
        let inside = start.checked_add(100_000_000).expect("time inside incident");
        for (sequence, time) in [end, inside, before, start].into_iter().enumerate() {
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: channel,
                        sequence: u32::try_from(sequence).expect("small sequence"),
                        log_time: time,
                        publish_time: time,
                    },
                    &[1],
                )
                .expect("record sample");
        }
        writer.finish().expect("finish MCAP");
        drop(writer);
        let player = Player::new(&path);
        assert!(player.is_open());
        let mut found = Vec::new();
        let count = player
            .for_each_item(&[], UtcRange::new(Some(start), Some(end)).expect("range"), |item| {
                found.push((item.log_time_nanos, item.delay_nanos));
                true
            })
            .expect("replay");
        assert_eq!(count, 2, "indexed={indexed}");
        assert_eq!(found, vec![(start, 0), (inside, 100_000_000)], "indexed={indexed}");
        std::fs::remove_file(path).expect("remove fixture");
    }
}
