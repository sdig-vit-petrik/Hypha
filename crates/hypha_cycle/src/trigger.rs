//! The trigger ports of a triggered cycle.
//!
//! A trigger port is named by its position in the cycle's trigger list, which holds the
//! trigger bindings in binding order.

/// A set of trigger ports.
///
/// It holds the ports below [`TriggerPorts::CAPACITY`], the most trigger ports a cycle has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TriggerPorts(u64);

impl TriggerPorts {
    /// The most trigger ports a cycle has, and so a set holds.
    pub const CAPACITY: usize = 64;

    /// This set with `port` added; `None` for a port at or past [`Self::CAPACITY`].
    #[must_use]
    pub fn with(self, port: usize) -> Option<Self> {
        bit(port).map(|bit| Self(self.0 | bit))
    }

    /// Whether `port` is in this set.
    #[must_use]
    pub fn contains(self, port: usize) -> bool {
        bit(port).is_some_and(|bit| self.0 & bit != 0)
    }
}

/// What the trigger ports of a triggered cycle hold in one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerStep {
    /// A step on the oldest pending sample of trigger port `port`; every other trigger
    /// port holds none.
    Sample {
        /// The port that fires.
        port: usize,
    },
    /// A step without a trigger sample: every trigger port is empty, and the ports in
    /// `due` read stale.
    Fallback {
        /// The ports whose own silence limit has passed.
        due: TriggerPorts,
    },
}

impl TriggerStep {
    /// Whether trigger port `port` takes its oldest pending sample in this step.
    #[must_use]
    pub const fn fires(self, port: usize) -> bool {
        matches!(self, Self::Sample { port: firing } if firing == port)
    }

    /// Whether trigger port `port` reads stale in this step.
    #[must_use]
    pub fn is_due(self, port: usize) -> bool {
        matches!(self, Self::Fallback { due } if due.contains(port))
    }
}

/// The bit of `port`, `None` past the capacity.
fn bit(port: usize) -> Option<u64> {
    u32::try_from(port).ok().and_then(|shift| 1_u64.checked_shl(shift))
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a test fails loudly on a set that refuses a port")]
mod tests {
    use super::TriggerPorts;

    #[test]
    fn a_set_holds_the_ports_below_its_capacity() {
        let set = TriggerPorts::default().with(0).and_then(|set| set.with(63));
        let set = set.expect("both ports fit");
        assert!(set.contains(0) && set.contains(63));
        assert!(!set.contains(1));
        assert_eq!(set.with(TriggerPorts::CAPACITY), None);
        assert!(!set.contains(TriggerPorts::CAPACITY));
    }
}
