//! The missed-period count of a timer wraps instead of stopping.

use hypha_core::timer::Timer;
use hypha_time::{Duration, Time};

#[test]
fn the_missed_count_wraps_after_the_maximum_value() {
    let far_future = Time::from_nanos(i64::MAX);
    let mut timer = Timer::new(Duration::from_nanos(1), Time::ZERO);
    assert!(timer.poll(far_future) > 0);
    let one_jump = timer.missed_count();
    assert!(one_jump > u64::MAX / 3, "three jumps must pass the maximum value");

    for _replay in 0..2 {
        assert_eq!(timer.poll(Time::ZERO), 0);
        assert!(timer.poll(far_future) > 0);
    }

    assert_eq!(timer.missed_count(), one_jump.wrapping_mul(3));
}
