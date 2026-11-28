//! How a slow-callback warning names its subject.

use super::Subject;

#[test]
fn a_slow_callback_is_named_by_its_topic_its_timer_period_or_its_cycle() {
    assert_eq!(
        Subject::Topic("/sensing/lidar/points").to_string(),
        "topic '/sensing/lidar/points'"
    );
    assert_eq!(Subject::Timer(100_000_000).to_string(), "timer 100 ms");
    assert_eq!(Subject::Timer(2_500_000).to_string(), "timer 2.5 ms");
    assert_eq!(Subject::Cycle("ekf").to_string(), "cycle 'ekf'");
    assert_eq!(Subject::Service("/reset").to_string(), "service '/reset'");
}
