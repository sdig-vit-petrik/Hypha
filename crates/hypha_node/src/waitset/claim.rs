//! The claim of a `WaitSet` by one thread at a time: a call or a run takes it with one
//! compare-and-swap of an owner token, so a second thread gets `InvalidState` (#83).

use std::sync::atomic::{AtomicU64, Ordering};

use super::WaitSetShared;
use crate::status::Error;

/// The source of the claim tokens; each thread takes one, never 0, on its first claim.
static NEXT_CLAIM_TOKEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static CLAIM_TOKEN: u64 = NEXT_CLAIM_TOKEN.fetch_add(1, Ordering::Relaxed);
}

/// The calling thread's claim token.
fn claim_token() -> u64 {
    CLAIM_TOKEN.with(|token| *token)
}

/// The claim of a `WaitSet` by the calling thread for one call or one run, released when
/// it drops. A claim inside a claim of the same thread, such as a callback's call during
/// the run, holds nothing more and releases nothing.
#[must_use = "the claim holds the WaitSet until it drops"]
pub struct WaitSetClaim<'a> {
    shared: &'a WaitSetShared,
    /// Whether this claim took the `WaitSet`, so its drop gives it back.
    took: bool,
}

impl Drop for WaitSetClaim<'_> {
    fn drop(&mut self) {
        if self.took {
            self.shared.owner.store(0, Ordering::Release);
        }
    }
}

impl WaitSetShared {
    /// Claim the `WaitSet` for the calling thread until the claim drops: one
    /// compare-and-swap of the owner, so of two threads that start a call or a run at once
    /// exactly one gets it. A C call claims before it forms a reference to the `WaitSet`,
    /// and a run claims for its whole length. Any thread may call it.
    ///
    /// # Errors
    /// `InvalidState` while another thread is inside a call or a run on this `WaitSet`;
    /// nothing is claimed then.
    pub fn claim(&self) -> Result<WaitSetClaim<'_>, Error> {
        let token = claim_token();
        match self.owner.compare_exchange(0, token, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(WaitSetClaim { shared: self, took: true }),
            Err(held) if held == token => Ok(WaitSetClaim { shared: self, took: false }),
            Err(_) => Err(Error::InvalidState),
        }
    }

    /// Whether a run is in progress on the calling thread: the run holds the claim for its
    /// whole length, so a call from one of its callbacks is the only one that sees it.
    pub(crate) fn runs_on_this_thread(&self) -> bool {
        self.running.is_running() && self.owner.load(Ordering::Acquire) == claim_token()
    }
}
