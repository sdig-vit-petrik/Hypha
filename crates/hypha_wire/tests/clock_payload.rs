//! A `rosgraph_msgs/Clock` payload: exactly one `time`, nanoseconds below one second.

use hypha_wire::ros_time::RosTime;

#[test]
fn a_clock_payload_decodes_to_its_time_and_nanoseconds() {
    let payload = [2, 0, 0, 0, 64, 66, 15, 0];
    let time = RosTime::decode_clock(&payload);
    assert_eq!(time, Some(RosTime::new(2, 1_000_000)));
    assert_eq!(time.map(RosTime::as_nanos), Some(2_001_000_000));
}

#[test]
fn the_largest_time_converts_without_overflow() {
    let time = RosTime::new(u32::MAX, 999_999_999);
    assert_eq!(time.as_nanos(), u64::from(u32::MAX) * 1_000_000_000 + 999_999_999);
}

#[test]
fn a_clock_payload_of_another_length_is_refused() {
    assert_eq!(RosTime::decode_clock(&[0; 7]), None);
    assert_eq!(RosTime::decode_clock(&[0; 9]), None);
    assert_eq!(RosTime::decode_clock(&[]), None);
}

#[test]
fn a_clock_payload_with_a_whole_second_of_nanoseconds_is_refused() {
    let one_second = [0, 0, 0, 0, 0, 202, 154, 59];
    assert_eq!(RosTime::decode_clock(&one_second), None);
    let just_below = [0, 0, 0, 0, 255, 201, 154, 59];
    assert_eq!(RosTime::decode_clock(&just_below), Some(RosTime::new(0, 999_999_999)));
}
