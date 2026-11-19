use super::*;

#[test]
fn a_live_frame_goes_stale_and_a_lasting_frame_does_not() {
    let now = Instant::now();
    let later = now.checked_add(Duration::from_secs(1)).unwrap_or(now);
    let live = QueuedFrame::live(vec![1], Duration::from_millis(500));
    assert!(!live.is_stale(now), "a fresh live frame is sent");
    assert!(live.is_stale(later), "a live frame older than its age limit is dropped");
    assert!(
        !QueuedFrame::lasting(vec![1]).is_stale(later),
        "an advertisement or latched replay is never dropped for age"
    );
}

fn tf_static(pairs: &[(&str, &str)], stamp_secs: u32) -> Vec<u8> {
    use hypha_node::sdk::message::EncodeInto;
    use hypha_node::sdk::msgs::geometry_msgs::TransformStamped;
    use hypha_node::sdk::msgs::tf2_msgs::TFMessage;
    let mut message = TFMessage::default();
    for (parent, child) in pairs {
        let mut transform = TransformStamped::default();
        transform.header.frame_id = (*parent).to_owned();
        transform.header.stamp.secs = stamp_secs;
        transform.child_frame_id = (*child).to_owned();
        message.transforms.push(transform);
    }
    let mut bytes = vec![0; message.encoded_len()];
    assert!(message.encode_into(&mut bytes).is_ok());
    bytes
}

#[test]
fn a_restamped_tf_static_replaces_its_latched_entry() {
    let mut latched = Vec::new();
    for stamp in 0..100 {
        merge_tf_static(
            &mut latched,
            u64::from(stamp),
            Arc::from(tf_static(&[("map", "odom")], stamp)),
        );
    }
    assert_eq!(latched.len(), 1, "one broadcaster re-stamping its pair keeps one entry");

    merge_tf_static(&mut latched, 200, Arc::from(tf_static(&[("base", "laser")], 1)));
    assert_eq!(latched.len(), 2, "a second broadcaster's pairs are kept beside the first");

    merge_tf_static(
        &mut latched,
        300,
        Arc::from(tf_static(&[("map", "odom"), ("base", "laser")], 2)),
    );
    assert_eq!(latched.len(), 1, "a message that covers every latched pair supersedes them");

    merge_tf_static(&mut latched, 400, Arc::from(vec![1, 2, 3]));
    assert_eq!(latched.len(), 1, "an undecodable payload is not latched");
}
