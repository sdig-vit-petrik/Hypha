use std::sync::Arc;

use super::{BindingSlot, NANOS_PER_SECOND};

/// Whether two bindings read the same history.
pub(super) fn shares_history(a: &BindingSlot, b: &BindingSlot) -> bool {
    match (&a.history, &b.history) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

pub(super) fn rotate_index(start: usize, offset: usize, length: usize) -> usize {
    let distance_to_end = length.saturating_sub(start);
    if offset >= distance_to_end {
        offset.saturating_sub(distance_to_end)
    } else {
        start.saturating_add(offset)
    }
}

/// A `RosTime` holds whole seconds in a `u32`. A negative stamp has negative
/// whole seconds, so it fails the same test.
pub(super) fn message_stamp_fits_ros_time(nanos: i64) -> bool {
    u32::try_from(nanos.div_euclid(NANOS_PER_SECOND)).is_ok()
}
