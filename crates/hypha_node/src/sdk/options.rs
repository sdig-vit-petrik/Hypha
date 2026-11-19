//! Configuration options for endpoints and resources.

pub use hypha_core::port::PayloadCapacity;

/// `QoS` used when opening a raw publisher. The SDK declares no service policy: the
/// topic's service is adopted, or created with hypha's `default` policy.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PublisherOptions {
    /// Whether late joiners receive retained samples.
    pub latch: bool,
    /// The largest message. A fixed-size type is bounded to the exact size of its
    /// encoded form. A variable-size type is growable by default.
    pub max_payload: PayloadCapacity,
}

impl PublisherOptions {
    /// Retain samples for late joiners.
    #[must_use]
    pub fn latched(mut self) -> Self {
        self.latch = true;
        self
    }

    /// Set late-joiner retention explicitly.
    #[must_use]
    pub fn with_latch(mut self, enabled: bool) -> Self {
        self.latch = enabled;
        self
    }

    /// Limit each message of a variable-size type to `bytes` of payload: the
    /// publisher's pool then needs `bytes` per sample of `/dev/shm`.
    #[must_use]
    pub fn bounded(mut self, bytes: usize) -> Self {
        self.max_payload = PayloadCapacity::Bounded(bytes);
        self
    }

    /// Remove the payload limit, for a relay that forwards messages of unknown
    /// size. Later segments are not checked against the free `/dev/shm`, and an
    /// undersized `/dev/shm` can stop the process with SIGBUS.
    #[must_use]
    pub fn growable(mut self) -> Self {
        self.max_payload = PayloadCapacity::Growable;
        self
    }

    /// [`Self::max_payload`] as the runtime takes it, or a message for a zero limit.
    pub(crate) fn checked_max_payload(self) -> Result<PayloadCapacity, String> {
        match self.max_payload {
            PayloadCapacity::Bounded(0) => Err("the payload capacity must be positive".to_string()),
            capacity => Ok(capacity),
        }
    }
}

/// `QoS` used when opening a raw subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriberOptions {
    /// Pending samples retained for this subscriber.
    pub depth: usize,
}

impl Default for SubscriberOptions {
    fn default() -> Self {
        Self { depth: 1 }
    }
}

impl SubscriberOptions {
    /// Keep `depth` pending samples.
    #[must_use]
    pub fn keeping(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }
}
