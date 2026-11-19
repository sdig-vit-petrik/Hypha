//! A timer observation reports whether the polled time was a rewind.

use hypha_core::timer::{Observation, Timer};
use hypha_time::{Duration, Time};

fn ms(millis: i64) -> Time {
    Time::from_nanos(millis.saturating_mul(1_000_000))
}

#[test]
fn an_observation_of_a_rewind_fires_nothing_and_reports_the_rewind() {
    let mut timer = Timer::new(Duration::from_millis(100), ms(1_000));

    assert_eq!(timer.observe(ms(500)), Observation { fired: 0, rewound: true });
    assert_eq!(timer.observe(ms(600)), Observation { fired: 1, rewound: false });
}

#[test]
fn an_observation_of_a_late_time_reports_every_expired_period() {
    let mut timer = Timer::new(Duration::from_millis(100), ms(0));

    assert_eq!(timer.observe(ms(350)), Observation { fired: 3, rewound: false });
    assert_eq!(timer.missed_count(), 2);
}

#[test]
fn a_timer_without_a_positive_period_reports_a_rewind_against_its_creation_time() {
    let mut timer = Timer::new(Duration::ZERO, ms(1_000));

    assert_eq!(timer.observe(ms(500)), Observation { fired: 0, rewound: true });
    assert_eq!(
        timer.observe(ms(999)),
        Observation { fired: 0, rewound: true },
        "the rewind did not move the recorded time"
    );
    assert_eq!(timer.observe(ms(1_000)), Observation { fired: 0, rewound: false });
}
