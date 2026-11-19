//! The smallest subscriber queue reported by a raw publisher.

use std::time::{Duration, Instant};

use hypha_node::sdk::BusNode;
use hypha_testkit::unique_name;

const TYPE_NAME: &str = "std_msgs/Bytes";
const MD5: &str = "f43a8e1b362b75baa741461b46adc7e0";

/// Polls `min_subscriber_buffer` until it equals `expected` or one second passes.
fn wait_for_min(
    publisher: &hypha_node::sdk::RawPublisher,
    expected: Option<usize>,
) -> Result<Option<usize>, String> {
    let Some(deadline) = Instant::now().checked_add(Duration::from_secs(1)) else {
        return Ok(None);
    };
    loop {
        let current = publisher.min_subscriber_buffer()?;
        if current == expected || Instant::now() >= deadline {
            return Ok(current);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn no_subscriber_reports_none() {
    let node = BusNode::new(&unique_name("min_none"));
    assert!(node.is_ok());
    let Ok(node) = node else { return };
    let topic = format!("/hypha/raw/{}", unique_name("min_none_topic"));
    let publisher = node.publish_raw(&topic, TYPE_NAME, MD5, "uint8[] data");
    assert!(publisher.is_ok());
    let Ok(publisher) = publisher else { return };

    assert_eq!(publisher.min_subscriber_buffer(), Ok(None));
}

#[test]
fn one_subscriber_reports_its_depth() {
    let node = BusNode::new(&unique_name("min_one"));
    assert!(node.is_ok());
    let Ok(node) = node else { return };
    let topic = format!("/hypha/raw/{}", unique_name("min_one_topic"));
    let publisher = node.publish_raw(&topic, TYPE_NAME, MD5, "uint8[] data");
    assert!(publisher.is_ok());
    let Ok(publisher) = publisher else { return };
    let subscriber = node.subscribe_raw(&topic, 4);
    assert!(subscriber.is_ok());
    let Ok(_subscriber) = subscriber else { return };

    assert_eq!(wait_for_min(&publisher, Some(4)), Ok(Some(4)));
}

#[test]
fn smallest_of_two_subscribers_is_reported() {
    let node = BusNode::new(&unique_name("min_two"));
    assert!(node.is_ok());
    let Ok(node) = node else { return };
    let topic = format!("/hypha/raw/{}", unique_name("min_two_topic"));
    let publisher = node.publish_raw(&topic, TYPE_NAME, MD5, "uint8[] data");
    assert!(publisher.is_ok());
    let Ok(publisher) = publisher else { return };
    let deep = node.subscribe_raw(&topic, 4);
    assert!(deep.is_ok());
    let Ok(_deep) = deep else { return };
    let shallow = node.subscribe_raw(&topic, 2);
    assert!(shallow.is_ok());
    let Ok(_shallow) = shallow else { return };

    assert_eq!(wait_for_min(&publisher, Some(2)), Ok(Some(2)));
}
