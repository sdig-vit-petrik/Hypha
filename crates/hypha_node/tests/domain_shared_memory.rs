//! The written shared memory of a domain: the allocated blocks of its objects on Linux,
//! and `None` on a system that does not tell it.

use hypha_node::sdk::BusNode;
use hypha_node::sdk::clean::domain_shared_memory;
use hypha_testkit::unique_name;

#[test]
#[expect(clippy::expect_used, reason = "a test fails on the first refused step")]
fn a_domain_with_a_publisher_holds_written_shared_memory_on_linux_and_none_elsewhere() {
    let bus = BusNode::new(&unique_name("shm_bytes_rust")).expect("node");
    let topic = format!("/{}", unique_name("shm_bytes"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    assert!(publisher.publish(&[7; 64]));
    let domain = hypha_node::sdk::domain().expect("domain");

    let written = domain_shared_memory(&domain).expect("read the domain");
    if cfg!(target_os = "linux") {
        assert!(written.is_some_and(|bytes| bytes > 0), "{domain}: {written:?}");
    } else {
        assert_eq!(written, None);
    }
}

#[test]
#[expect(clippy::expect_used, reason = "a test fails on the first refused step")]
fn a_domain_without_objects_holds_nothing_and_an_invalid_name_is_refused() {
    let unused = domain_shared_memory("zz_no_such_domain_here").expect("an unused domain");
    let expected = if cfg!(target_os = "linux") { Some(0) } else { None };
    assert_eq!(unused, expected);
    assert!(domain_shared_memory("Not-A-Domain").is_err());
}
