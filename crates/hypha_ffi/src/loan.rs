//! The C ABI wrappers of `hypha_node::loan`.

use crate::boundary::{handle_mut, handle_ref, take_box, write_out};
use crate::status::try_guard;
use hypha_node::safe::{HyphaLoan, HyphaPublisher, loan_send, loan_shrink, publisher_loan_frame};
use hypha_node::status::HyphaStatus;

/// Borrow a payload buffer of `payload_size` bytes for one sample.
///
/// On `Ok`, `*out` is the loan and `*out_payload` points to exactly `payload_size`
/// writable bytes, valid until the loan is sent or dropped; both are null on any other
/// status. The caller owns the loan and releases it with `hypha_loan_send` or
/// `hypha_loan_drop`. A null `publisher`, `out` or `out_payload` is `InvalidArgument`.
/// `InvalidState` once the node that made the publisher was destroyed
/// (`hypha_node_destroy`): nothing is sent on a node that has left the bus.
///
/// # Safety
/// `publisher` must come from `hypha_publish_with_qos`. `out` and `out_payload` each
/// point to one writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_publisher_loan_frame(
    publisher: *mut HyphaPublisher,
    payload_size: usize,
    out: *mut *mut HyphaLoan,
    out_payload: *mut *mut u8,
) -> HyphaStatus {
    try_guard(|| {
        let endpoint = unsafe { handle_ref(publisher) }?;
        unsafe {
            write_out(out, std::ptr::null_mut())?;
            write_out(out_payload, std::ptr::null_mut())?;
        }
        let mut held = publisher_loan_frame(endpoint, payload_size)?;
        let payload = held.payload_mut().ok_or(HyphaStatus::InvalidState)?.as_mut_ptr();
        unsafe {
            write_out(out, Box::into_raw(Box::new(held)))?;
            write_out(out_payload, payload)
        }
    })
}

/// Shorten a loan to its first `payload_size` bytes before it is sent, for a message
/// that came out shorter than the bytes it reserved.
///
/// The bytes kept stay where they are, and the payload pointer from
/// `hypha_publisher_loan_frame` stays valid; subscribers receive `payload_size` bytes.
/// The slot keeps its size in the pool; the frame header's `payload_len` carries the
/// shorter length, and the CRC covers exactly those bytes. A null `loan` and a `payload_size` longer than the
/// loan are `InvalidArgument`, and the loan is unchanged. It may be called more than once.
///
/// # Safety
/// `loan` must come from `hypha_publisher_loan_frame` and not yet be sent or dropped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_loan_shrink(
    loan: *mut HyphaLoan,
    payload_size: usize,
) -> HyphaStatus {
    try_guard(|| {
        let held = unsafe { handle_mut(loan) }?;
        loan_shrink(held, payload_size)?;
        Ok(())
    })
}

/// Send a filled loan. The loan is released whatever the result.
///
/// The framework send time is `framework_sent_nanos` when it is zero or more, else
/// framework time on the calling thread: the time of the cycle step or the replayed
/// callback that sends, else the framework clock. A null `loan` returns `InvalidArgument`.
/// `InvalidState`, and nothing sent, once the publisher's node was destroyed.
///
/// # Safety
/// `loan` must come from `hypha_publisher_loan_frame` and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_loan_send(
    loan: *mut HyphaLoan,
    framework_sent_nanos: i64,
) -> HyphaStatus {
    try_guard(|| {
        let held = unsafe { take_box(loan) }.ok_or(HyphaStatus::InvalidArgument)?;
        loan_send(*held, framework_sent_nanos)?;
        Ok(())
    })
}

/// Release a loan without sending it, as when serialisation fails. Null is allowed.
///
/// # Safety
/// `loan` must come from `hypha_publisher_loan_frame` and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_loan_drop(loan: *mut HyphaLoan) {
    drop(unsafe { take_box(loan) });
}
