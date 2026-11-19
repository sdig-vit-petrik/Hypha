//! Test helper: registers a node named argv[1] (publishing on argv[2], if given),
//! prints "ready", then blocks until it is killed.

#![allow(clippy::expect_used)]

fn main() {
    use std::io::Write as _;

    let name = std::env::args().nth(1).expect("crash_node_helper requires a node name argument");
    let transport = hypha_iceoryx2::Iox2Transport::with_name(&name).expect("create node");
    let _publisher = std::env::args().nth(2).map(|topic| {
        let spec = hypha_core::port::TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
        transport.publish(&spec).expect("create the publisher")
    });
    println!("ready");
    let _ = std::io::stdout().flush();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
