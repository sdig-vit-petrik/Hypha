//! Unit tests of the subscriber's absence watch and refusal report.

mod no_publisher_watch_tests {
    use super::super::{NoPublisherWatch, WatchDecision, schema_label};
    use hypha_iceoryx2::RecordedStream;
    use std::time::{Duration, Instant};

    fn schema(data_type: &str, md5: &str) -> RecordedStream {
        RecordedStream { schema_hash: 1, data_type: data_type.to_owned(), md5: md5.to_owned() }
    }

    #[test]
    fn an_absence_is_reported_after_the_delay_then_on_a_change_at_most_every_ten_seconds() {
        let start = Instant::now();
        let watch = NoPublisherWatch::new(Duration::from_secs(5), start);
        let at = |seconds| start + Duration::from_secs(seconds);
        let other = [schema("pkg/T", "581e4b1b")];
        assert_eq!(watch.observe(false, at(4)), WatchDecision::Quiet);
        assert_eq!(
            watch.observe(false, start + Duration::from_nanos(4_999_999_999)),
            WatchDecision::Quiet
        );
        assert_eq!(watch.observe(false, at(5)), WatchDecision::Absent);
        assert!(watch.report(&[], at(5)), "the first report");
        assert!(!watch.report(&[], at(20)), "the same absence is said once");
        assert!(watch.report(&other, at(21)), "another schema appeared");
        assert!(!watch.report(&[], at(22)), "a change within ten seconds waits");
        assert!(!watch.report(&other, at(30)), "and is not said when it went back");
        assert!(watch.report(&[], at(32)), "a change that lasts is said after the quiet time");
    }

    #[test]
    fn a_publisher_rearms_the_watch_and_zero_never_warns() {
        let start = Instant::now();
        let watch = NoPublisherWatch::new(Duration::from_secs(5), start);
        let at = |seconds| start + Duration::from_secs(seconds);
        assert_eq!(watch.observe(false, at(6)), WatchDecision::Absent);
        assert!(watch.report(&[], at(6)));
        assert_eq!(watch.observe(true, at(7)), WatchDecision::Quiet);
        assert_eq!(watch.observe(false, at(8)), WatchDecision::Quiet, "the delay starts again");
        assert_eq!(watch.observe(false, at(16)), WatchDecision::Absent);
        assert!(watch.report(&[], at(16)), "a new absence is reported again");

        let silent = NoPublisherWatch::new(Duration::ZERO, start);
        assert_eq!(silent.observe(false, at(100)), WatchDecision::Quiet);
        assert!(silent.due(1), "the stream is still checked");
    }

    #[test]
    fn the_stream_is_checked_once_per_second_or_once_per_shorter_delay() {
        let second = 1_000_000_000;
        let watch = NoPublisherWatch::new(Duration::from_secs(5), Instant::now());
        assert!(watch.due(7));
        assert!(!watch.due(7 + second - 1));
        assert!(watch.due(7 + second));

        let short = NoPublisherWatch::new(Duration::from_millis(100), Instant::now());
        assert!(short.due(7));
        assert!(!short.due(7 + 99_000_000));
        assert!(short.due(7 + 100_000_000));
    }

    #[test]
    fn a_schema_is_named_by_its_type_and_the_start_of_its_md5() {
        assert_eq!(
            schema_label("overload_test/First", "d9d213e8c146065ec958333c617eb1a6"),
            "overload_test/First (d9d213e8)"
        );
        assert_eq!(schema_label("", "d9d2"), "- (d9d2)");
        let hash = hypha_wire::schema_hash_from_md5("f79835def461350458ead76f07016dd7");
        let bare = RecordedStream {
            schema_hash: hash.unwrap_or(0),
            data_type: String::new(),
            md5: String::new(),
        };
        assert_eq!(super::super::stream_label(&bare), "- (f79835de)");
    }
}

mod rejection_report_tests {
    use super::super::RejectionReport;

    #[test]
    fn reports_the_first_refusal_then_once_per_interval_with_the_suppressed_count() {
        let report = RejectionReport::default();
        let second = RejectionReport::INTERVAL_NANOS;
        assert_eq!(report.due(5), Some(0));
        for _ in 0..99 {
            assert_eq!(report.due(5 + second - 1), None);
        }
        assert_eq!(report.due(5 + second), Some(99));
        assert_eq!(report.due(5 + second + 1), None);
        assert_eq!(report.due(5 + 3 * second), Some(1));
    }
}
