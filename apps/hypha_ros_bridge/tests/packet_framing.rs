//! Tests for the TCPROS outer length-prefix framing helper.

use hypha_ros_bridge::{add_ros_packet_length, strip_ros_packet};

#[test]
fn add_ros_packet_length_prepends_the_body_length() {
    let body = b"abc";
    let packet = add_ros_packet_length(body);
    assert!(matches!(packet, Ok(ref bytes) if bytes == &[3, 0, 0, 0, b'a', b'b', b'c']));
}

#[test]
fn add_ros_packet_length_accepts_an_empty_body() {
    let packet = add_ros_packet_length(&[]);
    assert!(matches!(packet, Ok(ref bytes) if bytes.as_slice() == [0, 0, 0, 0]));
}

#[test]
fn strip_ros_packet_removes_the_matching_body_length() {
    let body = strip_ros_packet(&[3, 0, 0, 0, b'a', b'b', b'c']);
    assert!(matches!(body, Ok(bytes) if bytes == b"abc"));
}

#[test]
fn strip_ros_packet_rejects_a_missing_or_mismatched_length() {
    assert!(strip_ros_packet(b"abc").is_err());
    assert!(strip_ros_packet(&[4, 0, 0, 0, b'a', b'b', b'c']).is_err());
}
