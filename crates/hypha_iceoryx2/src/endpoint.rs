//! The two endpoints, and the buffers they hand out.
//!
//! `new_set` resolves its streams once: matching costs O(services x nodes)
//! file-lock probes (175 ms with 120 publishers), so it is paid at open.

use std::sync::Arc;

use iceoryx2::port::{LoanError, SendError};
use iceoryx2::prelude::CallbackProgression;
use iceoryx2::service::port_factory::PortFactory;

use hypha_core::port::PortError;
use hypha_wire::frame::{
    FLAG_LATCHED, FrameHeader, FrameMetadata, MAX_PAYLOAD_LEN, TopicCode, message_bytes, seal,
};
use iceoryx2::port::notifier::Notifier;
use iceoryx2::port::publisher::Publisher;
use iceoryx2::port::update_connections::UpdateConnections;
use iceoryx2::sample::Sample;
use iceoryx2::sample_mut::SampleMut;

use crate::header::Iox2FrameHeader;
use crate::transport::{DataPortFactory, Ipc};

/// Backend code of a loan that failed inside iceoryx2 (`LoanError::InternalFailure`),
/// and of a size the publisher cannot name a limit for.
pub(crate) const BACKEND_LOAN_INTERNAL: u32 = 36;
/// Backend code of a send to a connection whose data is corrupted.
pub(crate) const BACKEND_SEND_CORRUPTED: u32 = 37;
/// Backend code of a send that could not establish a connection to a subscriber.
pub(crate) const BACKEND_SEND_CONNECTION: u32 = 38;
/// Backend code of a send that reached some subscribers but not all.
pub(crate) const BACKEND_SEND_UNDELIVERED: u32 = 39;
/// Backend code of a send that failed inside iceoryx2.
pub(crate) const BACKEND_SEND_INTERNAL: u32 = 40;

/// The status of a refused loan: a full pool is [`PortError::PoolExhausted`], the
/// publisher's own loan budget [`PortError::LoanLimit`]; every other refusal names its cause.
pub(crate) fn loan_port_error(
    cause: LoanError,
    requested: usize,
    max_payload: Option<usize>,
) -> PortError {
    match cause {
        LoanError::OutOfMemory => PortError::PoolExhausted,
        LoanError::ExceedsMaxLoans => PortError::LoanLimit,
        LoanError::ExceedsMaxLoanSize => match max_payload {
            Some(limit) => PortError::TooLarge { requested, limit },
            None => PortError::Backend(BACKEND_LOAN_INTERNAL),
        },
        LoanError::InternalFailure => PortError::Backend(BACKEND_LOAN_INTERNAL),
    }
}

/// The status of a failed send.
pub(crate) fn send_port_error(cause: SendError) -> PortError {
    match cause {
        SendError::ConnectionBrokenSinceSenderNoLongerExists => PortError::Disconnected,
        SendError::LoanError(loan) => loan_port_error(loan, 0, None),
        SendError::ConnectionError(_) => PortError::Backend(BACKEND_SEND_CONNECTION),
        SendError::ConnectionCorrupted => PortError::Backend(BACKEND_SEND_CORRUPTED),
        SendError::UnableToDeliver => PortError::Backend(BACKEND_SEND_UNDELIVERED),
        SendError::InternalError => PortError::Backend(BACKEND_SEND_INTERNAL),
    }
}

/// A slice of the shared segment, lent to write a message into. The slice is the
/// message payload alone; its frame header is written when the loan is sent.
pub struct Iox2Loan {
    sample: SampleMut<Ipc, [u8], Iox2FrameHeader>,
    /// The bytes of the message: the slice's length, or less after [`Self::shrink`]. The
    /// frame header carries it as `payload_len`; the slice keeps the loaned size.
    used: usize,
    /// The code of the publisher's topic, which the frame is sealed for.
    topic_code: TopicCode,
    notifier: Arc<Notifier<Ipc>>,
    /// True when the publisher latches its samples.
    latched: bool,
}

impl Iox2Loan {
    /// The bytes of the message currently in the buffer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        let payload = self.sample.payload();
        payload.get(..self.used).unwrap_or(payload)
    }

    /// The bytes to write the message into.
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        let used = self.used;
        let payload = self.sample.payload_mut();
        let whole = payload.len();
        payload.get_mut(..used.min(whole)).unwrap_or_default()
    }

    /// How many bytes the message holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.used
    }

    /// True when the buffer holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Shorten the message to its first `len` bytes before the send. The slot keeps
    /// its size; the frame's `payload_len` says `len`, and subscribers see `len` bytes.
    ///
    /// # Errors
    /// [`PortError::TooLarge`] when `len` is longer than the message, which is unchanged.
    pub fn shrink(&mut self, len: usize) -> Result<(), PortError> {
        if len > self.used {
            return Err(PortError::TooLarge { requested: len, limit: self.used });
        }
        self.used = len;
        Ok(())
    }

    /// Seal the payload with `metadata` and `flags`, then send it, consuming the
    /// loan. Dropping the loan without sending returns the slot to the publisher.
    /// A failed wake notification is logged and counted ([`crate::failed_notifications`]),
    /// not returned: the sample is visible, so a retry would send it twice.
    ///
    /// # Errors
    /// The error of the failed send, see [`send_port_error`].
    pub fn send_frame(mut self, metadata: FrameMetadata, flags: u32) -> Result<(), PortError> {
        let flags = if self.latched { flags | FLAG_LATCHED } else { flags };
        let header = seal(self.topic_code, metadata, flags, self.as_bytes());
        *self.sample.user_header_mut() = Iox2FrameHeader::from(header);
        self.sample.send().map_err(send_port_error)?;
        if !crate::notification::notified(&self.notifier.notify()) {
            log::warn!(target: "hypha/transport", "hypha_iceoryx2: data was sent but its wake notification failed");
        }
        Ok(())
    }
}

/// One sample, read where the publisher wrote it.
pub struct Iox2Sample {
    pub(crate) topic: Arc<str>,
    pub(crate) sample: Sample<Ipc, [u8], Iox2FrameHeader>,
}

impl Iox2Sample {
    /// The topic this sample arrived on.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// The code of the topic this sample arrived on, which its frame must be sealed for.
    #[must_use]
    pub fn topic_code(&self) -> TopicCode {
        TopicCode::of(&self.topic)
    }

    /// The frame header the publisher sealed. Verify it with
    /// [`hypha_wire::frame::verify`] against [`Self::topic_code`] and [`Self::payload`].
    #[must_use]
    pub fn header(&self) -> FrameHeader {
        FrameHeader::from(*self.sample.user_header())
    }

    /// The message bytes, read where the publisher wrote them: the first `payload_len`
    /// bytes of the slice. A header that claims more gives the whole slice, which
    /// [`hypha_wire::frame::verify`] refuses.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        message_bytes(&self.header(), self.sample.payload())
    }
}

/// Sends samples on one topic.
pub struct Iox2Publisher {
    /// Shared with the latch registry of a latched topic.
    pub(crate) publisher: Arc<Publisher<Ipc, [u8], Iox2FrameHeader>>,
    /// The topic's service. Its dynamic config lists the subscriber ports.
    pub(crate) service: Arc<DataPortFactory>,
    pub(crate) topic: String,
    /// The code of `topic`, which every frame of this publisher is sealed for.
    pub(crate) topic_code: TopicCode,
    pub(crate) notifier: Arc<Notifier<Ipc>>,
    /// True when this publisher latches its samples.
    pub(crate) latched: bool,
    /// The largest loan, or `None` for a growable publisher.
    pub(crate) max_payload: Option<usize>,
    /// The current run of loans the pool refused; a lock so a loan takes `&self`.
    pub(crate) pool_refusals: std::sync::Mutex<crate::refusal_episode::RefusalEpisode>,
    /// True while a refusal run is open, so a granted loan outside one takes no lock.
    pub(crate) pool_refusing: std::sync::atomic::AtomicBool,
    /// After the port and the service handle, so the record goes when no port is left.
    pub(crate) _schema_release: crate::schema::SchemaRelease,
    /// Last: the graph event of its drop follows the port's.
    pub(crate) _graph: crate::transport::GraphAnnouncement,
}

impl Iox2Publisher {
    /// The largest message this publisher sends, or `None` for a growable publisher.
    #[must_use]
    pub fn max_payload(&self) -> Option<usize> {
        self.max_payload
    }

    /// Borrow a buffer of `size` bytes to write a message into.
    ///
    /// # Errors
    /// [`PortError::TooLarge`] when `size` exceeds the payload capacity or what a frame's
    /// `payload_len` can say ([`MAX_PAYLOAD_LEN`], 4 GiB − 1 bytes),
    /// [`PortError::PoolExhausted`] when no buffer is free, [`PortError::LoanLimit`] when
    /// this publisher's callers hold all its loans; any other refusal names its cause. A
    /// publisher never waits for a slow subscriber.
    pub fn loan(&self, size: usize) -> Result<Iox2Loan, PortError> {
        let limit = self.max_payload.map_or(MAX_PAYLOAD_LEN, |limit| limit.min(MAX_PAYLOAD_LEN));
        if size > limit {
            return Err(PortError::TooLarge { requested: size, limit });
        }
        let loaned = self.publisher.loan_slice(size);
        let pool_refused = matches!(loaned, Err(iceoryx2::port::LoanError::OutOfMemory));
        let tracked = pool_refused
            || (loaned.is_ok() && self.pool_refusing.load(std::sync::atomic::Ordering::Relaxed));
        let persists = tracked && {
            self.pool_refusing.store(pool_refused, std::sync::atomic::Ordering::Relaxed);
            crate::locked(&self.pool_refusals).observe(pool_refused, std::time::Instant::now)
        };
        if persists {
            log::warn!(target: "hypha/transport",
                "hypha: topic '{}' refuses loans although this publisher is below its loan limit; its pool \
                 may be exhausted by slow subscribers or corrupted. If it persists, restart the node.",
                self.topic);
        }
        loaned
            .map(|sample| Iox2Loan {
                sample,
                used: size,
                topic_code: self.topic_code,
                notifier: self.notifier.clone(),
                latched: self.latched,
            })
            .map_err(|cause| loan_port_error(cause, size, self.max_payload))
    }

    /// Refresh connections to subscribers.
    ///
    /// # Errors
    /// [`PortError::Disconnected`] when the endpoint cannot refresh its links.
    pub fn update_connections(&self) -> Result<(), PortError> {
        self.publisher.update_connections().map_err(|_| PortError::Disconnected)
    }

    /// The number of subscriber ports on this topic on this machine, from any transport.
    #[must_use]
    pub fn subscriber_count(&self) -> Option<usize> {
        Some(self.service.dynamic_config().number_of_subscribers())
    }

    /// The smallest queue among the subscribers connected to this topic, or
    /// `None` when none is connected.
    #[must_use]
    pub fn min_subscriber_buffer(&self) -> Option<usize> {
        let mut smallest: Option<usize> = None;
        self.service.dynamic_config().list_subscribers(|details| {
            smallest = Some(smallest.map_or(details.buffer_size, |s| s.min(details.buffer_size)));
            CallbackProgression::Continue
        });
        smallest
    }

    /// The topic this publisher sends on.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }
}

#[cfg(test)]
mod tests {
    use super::{BACKEND_LOAN_INTERNAL, BACKEND_SEND_INTERNAL, loan_port_error, send_port_error};
    use hypha_core::port::PortError;
    use iceoryx2::port::{LoanError, SendError};

    #[test]
    fn a_full_pool_and_the_loan_budget_are_pool_exhaustion() {
        assert_eq!(loan_port_error(LoanError::OutOfMemory, 8, None), PortError::PoolExhausted);
        assert_eq!(loan_port_error(LoanError::ExceedsMaxLoans, 8, None), PortError::LoanLimit);
    }

    #[test]
    fn a_loan_over_the_publisher_limit_is_too_large_and_names_the_limit() {
        assert_eq!(
            loan_port_error(LoanError::ExceedsMaxLoanSize, 16, Some(8)),
            PortError::TooLarge { requested: 16, limit: 8 }
        );
        assert_eq!(
            loan_port_error(LoanError::ExceedsMaxLoanSize, 16, None),
            PortError::Backend(BACKEND_LOAN_INTERNAL),
            "a growable publisher has no limit to name"
        );
    }

    #[test]
    fn an_internal_loan_failure_is_a_backend_failure() {
        assert_eq!(
            loan_port_error(LoanError::InternalFailure, 8, None),
            PortError::Backend(BACKEND_LOAN_INTERNAL)
        );
    }

    #[test]
    fn a_send_to_a_publisher_that_is_gone_is_disconnected_and_others_name_their_cause() {
        assert_eq!(
            send_port_error(SendError::ConnectionBrokenSinceSenderNoLongerExists),
            PortError::Disconnected
        );
        assert_eq!(
            send_port_error(SendError::InternalError),
            PortError::Backend(BACKEND_SEND_INTERNAL)
        );
        assert_eq!(
            send_port_error(SendError::LoanError(LoanError::OutOfMemory)),
            PortError::PoolExhausted
        );
    }
}
