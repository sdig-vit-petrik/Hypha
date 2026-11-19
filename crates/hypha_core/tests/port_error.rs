//! Tests for `PortError` values and traits.

use hypha_core::port::PortError;

fn assert_copy<T: Copy>() {}
fn assert_error<T: core::error::Error>() {}

#[test]
fn port_error_has_standard_error_traits() {
    assert_copy::<PortError>();
    assert_error::<PortError>();
    let error = PortError::Backend(7);
    let copied = error;
    assert_eq!(error, copied);
}

#[test]
fn port_error_formats_each_variant() {
    let cases = [
        (PortError::PoolExhausted, "no free buffer; the message is dropped"),
        (PortError::BorrowLimit, "the subscriber holds its maximum loan budget"),
        (
            PortError::TooLarge { requested: 9, limit: 8 },
            "a buffer of 9 bytes is over the 8 byte limit",
        ),
        (PortError::Disconnected, "the endpoint is closed"),
        (PortError::NotAvailable, "the requested endpoint is not available"),
        (PortError::ResourceLimit, "the endpoint resource limit was reached"),
        (PortError::InvalidTopic, "the topic name was refused"),
        (PortError::InvalidArgument, "the arguments cannot be used together"),
        (PortError::Backend(7), "the transport failed with code 7"),
    ];

    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}
