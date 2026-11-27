//! What a call reports back.

use hypha_core::port::PortError;

/// What a call reports back.
///
/// Only [`HyphaStatus::Ok`] means an output parameter was filled.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyphaStatus {
    /// The call succeeded.
    Ok = 0,
    /// A pointer was null, or a length made no sense.
    InvalidArgument = 1,
    /// The topic name was empty or refused.
    InvalidTopic = 2,
    /// No buffer was free. The caller drops this message.
    PoolExhausted = 3,
    /// The subscriber holds its maximum number of received samples.
    BorrowLimit = 8,
    /// The requested buffer is larger than the transport carries.
    TooLarge = 4,
    /// The endpoint is closed.
    Disconnected = 5,
    /// The transport failed for a reason of its own.
    Backend = 6,
    /// A foreign callback threw, or returned this status. The call did nothing.
    Panic = 7,
    /// The received-sample frame is not an hypha frame.
    InvalidFrame = 9,
    /// Owning receive and `WaitSet` dispatch were mixed on one subscriber.
    InvalidOperation = 10,
    /// A required service, schema record, or capability was not available.
    NotAvailable = 11,
    /// A schema record failed parsing, validation, or hash checking.
    InvalidSchema = 12,
    /// An operation was rejected because a fixed resource limit was reached.
    ResourceLimit = 13,
    /// The frame CRC-32C does not match the sample header, payload and topic.
    CorruptPayload = 15,
    /// The frame sequence was not newer than the last one from its publisher.
    SequenceGap = 16,
    /// Static endpoint `QoS` does not match the topic service.
    QosMismatch = 17,
    /// Monotonic or simulation timeout elapsed.
    Timeout = 18,
    /// Operation invoked in invalid state.
    InvalidState = 20,
    /// A `/clock` sample went below the node's last one: a run of a `WaitSet` on simulated
    /// time ends with it, whatever it dispatches (cycles, plain timers, plain
    /// subscriptions); a command-line tool's node is exempt. The process restarts.
    ClockRewound = 22,
    /// The application asked for its run to fail (`hypha_waitset_request_failure`).
    ApplicationFailed = 23,
    /// The publisher's callers already hold all its loans (its loan count). Nothing was
    /// taken from the pool: send or drop a held loan, then ask again.
    LoanLimit = 24,
}

impl HyphaStatus {
    /// Convert an integer error code back into an [`HyphaStatus`].
    #[must_use]
    pub const fn from_i32(val: i32) -> Self {
        match val {
            0 => Self::Ok,
            1 => Self::InvalidArgument,
            2 => Self::InvalidTopic,
            3 => Self::PoolExhausted,
            4 => Self::TooLarge,
            5 => Self::Disconnected,
            7 => Self::Panic,
            8 => Self::BorrowLimit,
            9 => Self::InvalidFrame,
            10 => Self::InvalidOperation,
            11 => Self::NotAvailable,
            12 => Self::InvalidSchema,
            13 => Self::ResourceLimit,
            15 => Self::CorruptPayload,
            16 => Self::SequenceGap,
            17 => Self::QosMismatch,
            18 => Self::Timeout,
            20 => Self::InvalidState,
            22 => Self::ClockRewound,
            23 => Self::ApplicationFailed,
            24 => Self::LoanLimit,
            _ => Self::Backend,
        }
    }
}

/// Why a safe call of this crate failed: one variant per status but [`HyphaStatus::Ok`].
///
/// Each variant has the number of its status, so `error as i32` is the status code too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Error {
    /// See [`HyphaStatus::InvalidArgument`].
    InvalidArgument = 1,
    /// See [`HyphaStatus::InvalidTopic`].
    InvalidTopic = 2,
    /// See [`HyphaStatus::PoolExhausted`].
    PoolExhausted = 3,
    /// See [`HyphaStatus::BorrowLimit`].
    BorrowLimit = 8,
    /// See [`HyphaStatus::TooLarge`].
    TooLarge = 4,
    /// See [`HyphaStatus::Disconnected`].
    Disconnected = 5,
    /// See [`HyphaStatus::Backend`].
    Backend = 6,
    /// See [`HyphaStatus::Panic`].
    Panic = 7,
    /// See [`HyphaStatus::InvalidFrame`].
    InvalidFrame = 9,
    /// See [`HyphaStatus::InvalidOperation`].
    InvalidOperation = 10,
    /// See [`HyphaStatus::NotAvailable`].
    NotAvailable = 11,
    /// See [`HyphaStatus::InvalidSchema`].
    InvalidSchema = 12,
    /// See [`HyphaStatus::ResourceLimit`].
    ResourceLimit = 13,
    /// See [`HyphaStatus::CorruptPayload`].
    CorruptPayload = 15,
    /// See [`HyphaStatus::SequenceGap`].
    SequenceGap = 16,
    /// See [`HyphaStatus::QosMismatch`].
    QosMismatch = 17,
    /// See [`HyphaStatus::Timeout`].
    Timeout = 18,
    /// See [`HyphaStatus::InvalidState`].
    InvalidState = 20,
    /// See [`HyphaStatus::ClockRewound`].
    ClockRewound = 22,
    /// See [`HyphaStatus::ApplicationFailed`].
    ApplicationFailed = 23,
    /// See [`HyphaStatus::LoanLimit`].
    LoanLimit = 24,
}

impl Error {
    /// The error a status of a lower layer stands for. A status that is not an
    /// error is `Backend`: the lower layer broke its contract.
    pub(crate) const fn from_status(status: HyphaStatus) -> Self {
        Self::from_code(status as i32)
    }

    /// See [`Error::from_status`]; `code` is a `HyphaStatus` value.
    pub(crate) const fn from_code(code: i32) -> Self {
        match HyphaStatus::from_i32(code) {
            HyphaStatus::Ok | HyphaStatus::Backend => Self::Backend,
            HyphaStatus::InvalidArgument => Self::InvalidArgument,
            HyphaStatus::InvalidTopic => Self::InvalidTopic,
            HyphaStatus::PoolExhausted => Self::PoolExhausted,
            HyphaStatus::BorrowLimit => Self::BorrowLimit,
            HyphaStatus::TooLarge => Self::TooLarge,
            HyphaStatus::Disconnected => Self::Disconnected,
            HyphaStatus::Panic => Self::Panic,
            HyphaStatus::InvalidFrame => Self::InvalidFrame,
            HyphaStatus::InvalidOperation => Self::InvalidOperation,
            HyphaStatus::NotAvailable => Self::NotAvailable,
            HyphaStatus::InvalidSchema => Self::InvalidSchema,
            HyphaStatus::ResourceLimit => Self::ResourceLimit,
            HyphaStatus::CorruptPayload => Self::CorruptPayload,
            HyphaStatus::SequenceGap => Self::SequenceGap,
            HyphaStatus::QosMismatch => Self::QosMismatch,
            HyphaStatus::Timeout => Self::Timeout,
            HyphaStatus::InvalidState => Self::InvalidState,
            HyphaStatus::ClockRewound => Self::ClockRewound,
            HyphaStatus::ApplicationFailed => Self::ApplicationFailed,
            HyphaStatus::LoanLimit => Self::LoanLimit,
        }
    }
}

impl From<Error> for HyphaStatus {
    fn from(error: Error) -> Self {
        match error {
            Error::InvalidArgument => Self::InvalidArgument,
            Error::InvalidTopic => Self::InvalidTopic,
            Error::PoolExhausted => Self::PoolExhausted,
            Error::BorrowLimit => Self::BorrowLimit,
            Error::TooLarge => Self::TooLarge,
            Error::Disconnected => Self::Disconnected,
            Error::Backend => Self::Backend,
            Error::Panic => Self::Panic,
            Error::InvalidFrame => Self::InvalidFrame,
            Error::InvalidOperation => Self::InvalidOperation,
            Error::NotAvailable => Self::NotAvailable,
            Error::InvalidSchema => Self::InvalidSchema,
            Error::ResourceLimit => Self::ResourceLimit,
            Error::CorruptPayload => Self::CorruptPayload,
            Error::SequenceGap => Self::SequenceGap,
            Error::QosMismatch => Self::QosMismatch,
            Error::Timeout => Self::Timeout,
            Error::InvalidState => Self::InvalidState,
            Error::ClockRewound => Self::ClockRewound,
            Error::ApplicationFailed => Self::ApplicationFailed,
            Error::LoanLimit => Self::LoanLimit,
        }
    }
}

impl From<PortError> for Error {
    fn from(cause: PortError) -> Self {
        match cause {
            PortError::PoolExhausted => Self::PoolExhausted,
            PortError::BorrowLimit => Self::BorrowLimit,
            PortError::LoanLimit => Self::LoanLimit,
            PortError::TooLarge { .. } => Self::TooLarge,
            PortError::Disconnected => Self::Disconnected,
            PortError::NotAvailable => Self::NotAvailable,
            PortError::ResourceLimit => Self::ResourceLimit,
            PortError::QosMismatch => Self::QosMismatch,
            PortError::InvalidTopic => Self::InvalidTopic,
            PortError::Timeout => Self::Timeout,
            PortError::Backend(_) => Self::Backend,
            PortError::InvalidState => Self::InvalidState,
            PortError::InvalidArgument | PortError::InvalidDomain => Self::InvalidArgument,
        }
    }
}

impl From<PortError> for HyphaStatus {
    fn from(cause: PortError) -> Self {
        Self::from(Error::from(cause))
    }
}

/// The status of a safe call's result: `Ok`, or the status of its error.
#[must_use]
pub fn status_of(result: Result<(), Error>) -> HyphaStatus {
    match result {
        Ok(()) => HyphaStatus::Ok,
        Err(error) => HyphaStatus::from(error),
    }
}

#[cfg(test)]
mod error_code_tests {
    use super::{Error, HyphaStatus};

    const EVERY_ERROR: [Error; 21] = [
        Error::InvalidArgument,
        Error::InvalidTopic,
        Error::PoolExhausted,
        Error::BorrowLimit,
        Error::TooLarge,
        Error::Disconnected,
        Error::Backend,
        Error::Panic,
        Error::InvalidFrame,
        Error::InvalidOperation,
        Error::NotAvailable,
        Error::InvalidSchema,
        Error::ResourceLimit,
        Error::CorruptPayload,
        Error::SequenceGap,
        Error::QosMismatch,
        Error::Timeout,
        Error::InvalidState,
        Error::ClockRewound,
        Error::ApplicationFailed,
        Error::LoanLimit,
    ];

    #[test]
    fn an_error_cast_to_a_number_is_its_status_code() {
        for error in EVERY_ERROR {
            assert_eq!(error as i32, HyphaStatus::from(error) as i32, "{error:?}");
            assert_eq!(Error::from_code(error as i32), error, "{error:?}");
        }
    }
}
