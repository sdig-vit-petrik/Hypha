//! The status mapping and the schema boundary checks at the C ABI.
#![allow(clippy::expect_used)]

use hypha_ffi::*;

/// Every status this build defines, with the number a C caller sees.
const EVERY_STATUS: [(i32, HyphaStatus); 22] = [
    (0, HyphaStatus::Ok),
    (1, HyphaStatus::InvalidArgument),
    (2, HyphaStatus::InvalidTopic),
    (3, HyphaStatus::PoolExhausted),
    (4, HyphaStatus::TooLarge),
    (5, HyphaStatus::Disconnected),
    (6, HyphaStatus::Backend),
    (7, HyphaStatus::Panic),
    (8, HyphaStatus::BorrowLimit),
    (9, HyphaStatus::InvalidFrame),
    (10, HyphaStatus::InvalidOperation),
    (11, HyphaStatus::NotAvailable),
    (12, HyphaStatus::InvalidSchema),
    (13, HyphaStatus::ResourceLimit),
    (15, HyphaStatus::CorruptPayload),
    (16, HyphaStatus::SequenceGap),
    (17, HyphaStatus::QosMismatch),
    (18, HyphaStatus::Timeout),
    (20, HyphaStatus::InvalidState),
    (22, HyphaStatus::ClockRewound),
    (23, HyphaStatus::ApplicationFailed),
    (24, HyphaStatus::LoanLimit),
];

#[test]
fn every_status_keeps_its_number_in_both_directions() {
    for (code, status) in EVERY_STATUS {
        assert_eq!(status as i32, code, "{status:?} changed its number");
        assert_eq!(HyphaStatus::from_i32(code), status, "code {code} decoded to the wrong status");
    }
}

#[test]
fn an_unknown_code_becomes_a_backend_failure() {
    // A number this build does not define must not decode to a status whose
    // meaning the caller would then act on.
    for code in [-1, 14, 19, 21, 25, 99, i32::MAX, i32::MIN] {
        assert_eq!(HyphaStatus::from_i32(code), HyphaStatus::Backend, "code {code}");
    }
}

#[test]
fn a_transport_failure_maps_to_the_status_that_names_it() {
    use hypha_core::port::PortError;
    let mapped = [
        (PortError::PoolExhausted, HyphaStatus::PoolExhausted),
        (PortError::BorrowLimit, HyphaStatus::BorrowLimit),
        (PortError::LoanLimit, HyphaStatus::LoanLimit),
        (PortError::TooLarge { limit: 8, requested: 16 }, HyphaStatus::TooLarge),
        (PortError::Disconnected, HyphaStatus::Disconnected),
        (PortError::NotAvailable, HyphaStatus::NotAvailable),
        (PortError::ResourceLimit, HyphaStatus::ResourceLimit),
        (PortError::QosMismatch, HyphaStatus::QosMismatch),
        (PortError::InvalidTopic, HyphaStatus::InvalidTopic),
        (PortError::Timeout, HyphaStatus::Timeout),
        (PortError::Backend(7), HyphaStatus::Backend),
        (PortError::InvalidState, HyphaStatus::InvalidState),
    ];
    for (cause, expected) in mapped {
        assert_eq!(HyphaStatus::from(cause), expected, "{cause:?} mapped wrongly");
    }
}
