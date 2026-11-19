//! `HYPHA_BOUNDED_MEMORY` accepts `1` and `0` alone. Any other value refuses the
//! domain, so no node starts with a setting it only seems to have.
#![allow(clippy::expect_used)]

use hypha_core::port::PortError;
use hypha_iceoryx2::Iox2Transport;

#[test]
fn an_unknown_value_refuses_the_node() {
    hypha_testkit::set_env(hypha_iceoryx2::domain::BOUNDED_MEMORY_VARIABLE, "true");

    let refused = Iox2Transport::new();

    assert!(matches!(refused, Err(PortError::InvalidArgument)), "the node is refused");
    assert_eq!(hypha_iceoryx2::domain::bounded_memory(), Err(PortError::InvalidArgument));
}
