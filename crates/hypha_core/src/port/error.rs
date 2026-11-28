//! What a transport call can refuse to do.

use core::fmt;

/// Why a port call did not succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortError {
    /// The transport has no free buffer. A publisher never blocks for a slow subscriber.
    PoolExhausted,
    /// The subscriber has reached its configured owning-sample budget.
    BorrowLimit,
    /// The publisher's callers hold all the loans it may lend at once; the pool may have
    /// free buffers.
    LoanLimit,
    /// The requested buffer is larger than the transport can carry.
    TooLarge {
        /// Bytes the caller asked for.
        requested: usize,
        /// The largest a single buffer may be.
        limit: usize,
    },
    /// The endpoint is closed, or the session behind it has gone.
    Disconnected,
    /// The requested endpoint does not exist.
    NotAvailable,
    /// A fixed endpoint or command limit was reached.
    ResourceLimit,
    /// An endpoint requested static service settings that do not match.
    QosMismatch,
    /// The topic name is empty or the transport refused it.
    InvalidTopic,
    /// The arguments cannot be used together.
    InvalidArgument,
    /// Monotonic or relative timeout elapsed.
    Timeout,
    /// The transport failed for a reason of its own. The code is the backend's.
    Backend(u32),
    /// The environment selects a domain whose name is not valid.
    InvalidDomain,
    /// The domain's shared files cannot be used by this process: another user owns them,
    /// or this process sees other mounts.
    InvalidState,
}

impl core::error::Error for PortError {}

impl fmt::Display for PortError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolExhausted => write!(out, "no free buffer; the message is dropped"),
            Self::BorrowLimit => write!(out, "the subscriber holds its maximum loan budget"),
            Self::LoanLimit => write!(out, "the publisher's callers hold all its loans"),
            Self::TooLarge { requested, limit } => {
                write!(out, "a buffer of {requested} bytes is over the {limit} byte limit")
            }
            Self::Disconnected => write!(out, "the endpoint is closed"),
            Self::NotAvailable => write!(out, "the requested endpoint is not available"),
            Self::ResourceLimit => write!(out, "the endpoint resource limit was reached"),
            Self::QosMismatch => write!(out, "the endpoint QoS does not match the service"),
            Self::InvalidTopic => write!(out, "the topic name was refused"),
            Self::InvalidArgument => write!(out, "the arguments cannot be used together"),
            Self::Timeout => write!(out, "the operation timed out"),
            Self::Backend(code) => write!(out, "the transport failed with code {code}"),
            Self::InvalidDomain => write!(out, "the selected domain name is not valid"),
            Self::InvalidState => {
                write!(out, "the domain's shared files cannot be used by this process")
            }
        }
    }
}
