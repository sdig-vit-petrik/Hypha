//! A subscriber names the alive nodes that publish on its topic.

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

#[allow(clippy::expect_used)]
fn publisher_nodes(subscriber: &hypha_iceoryx2::Iox2Subscriber) -> Vec<String> {
    let mut ids = Vec::new();
    subscriber.publisher_node_ids(&mut ids);
    let mut names: Vec<String> =
        hypha_iceoryx2::node_names(&ids).expect("names").into_values().collect();
    names.sort();
    names.dedup();
    names
}

#[test]
#[allow(clippy::expect_used)]
fn subscriber_names_the_node_that_publishes_and_forgets_it_after_the_publisher_drops() {
    let publisher_node = unique_name("pn_pub");
    let publishing = Iox2Transport::with_name(&publisher_node).expect("publishing transport");
    let listening = Iox2Transport::with_name(&unique_name("pn_sub")).expect("listening transport");
    let topic = format!("/{}", unique_name("pn_topic"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);

    // No publisher exists yet, so this subscriber declares the service it creates.
    let declared = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let declared = declared.with_qos(QosRequest::Declared(QosPolicy::new("pn", 8, 16, 8)));
    let subscriber = listening.subscribe(&declared).expect("subscriber");
    assert_eq!(publisher_nodes(&subscriber), Vec::<String>::new());

    let publisher = publishing.publish(&spec).expect("publisher");
    assert_eq!(publisher_nodes(&subscriber), vec![publisher_node.clone()]);

    drop(publisher);
    assert_eq!(publisher_nodes(&subscriber), Vec::<String>::new());
}

#[test]
#[allow(clippy::expect_used)]
fn two_publishers_on_one_topic_are_named_once_each_in_sorted_order() {
    let first_node = unique_name("pn_first");
    let second_node = unique_name("pn_second");
    let first = Iox2Transport::with_name(&first_node).expect("first transport");
    let second = Iox2Transport::with_name(&second_node).expect("second transport");
    let listening = Iox2Transport::with_name(&unique_name("pn_listen")).expect("listening");
    let topic = format!("/{}", unique_name("pn_two"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);

    let _first_publisher = first.publish(&spec).expect("first publisher");
    let _second_publisher = second.publish(&spec).expect("second publisher");
    let subscriber = listening.subscribe(&spec).expect("subscriber");
    let mut expected = vec![first_node, second_node];
    expected.sort();
    assert_eq!(publisher_nodes(&subscriber), expected);
}

#[test]
#[allow(clippy::expect_used)]
fn a_publisher_that_leaves_and_another_that_joins_change_the_ids_at_equal_count() {
    let leaving_node = unique_name("pn_leave");
    let joining_node = unique_name("pn_join");
    let leaving = Iox2Transport::with_name(&leaving_node).expect("leaving transport");
    let joining = Iox2Transport::with_name(&joining_node).expect("joining transport");
    let listening = Iox2Transport::with_name(&unique_name("pn_watch")).expect("listening");
    let topic = format!("/{}", unique_name("pn_swap"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);

    let first = leaving.publish(&spec).expect("first publisher");
    let subscriber = listening.subscribe(&spec).expect("subscriber");
    let mut before = Vec::new();
    subscriber.publisher_node_ids(&mut before);
    assert_eq!(before.len(), 1);
    drop(first);
    let second = joining.publish(&spec).expect("second publisher");
    let mut after = Vec::new();
    subscriber.publisher_node_ids(&mut after);
    assert_eq!(after.len(), 1, "the count stays at one");
    assert_ne!(before, after, "the set of publisher nodes changed");
    assert_eq!(publisher_nodes(&subscriber), vec![joining_node]);
    drop(second);
}
