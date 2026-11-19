//! Loan budget tracking for a subscriber's outstanding borrowed samples.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub(crate) struct LoanBudget {
    pub(crate) budget: usize,
    pub(crate) held: AtomicUsize,
    pub(crate) refused: AtomicU64,
}

impl LoanBudget {
    pub(crate) fn reserve(&self) -> bool {
        let mut current = self.held.load(Ordering::Acquire);
        loop {
            if current >= self.budget {
                return false;
            }
            let next = current.saturating_add(1);
            match self.held.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    pub(crate) fn release(&self) {
        let mut current = self.held.load(Ordering::Acquire);
        while current > 0 {
            match self.held.compare_exchange_weak(
                current,
                current.saturating_sub(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }
}
