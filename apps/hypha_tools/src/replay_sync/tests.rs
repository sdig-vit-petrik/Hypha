use super::*;

fn report(node: &str, clock_nanos: u64, sent_nanos: u64) -> Progress {
    Progress { node: node.to_string(), clock_nanos, clock_sent_nanos: sent_nanos }
}

#[test]
fn a_progress_report_decodes_from_its_wire_bytes() {
    // "ab" as a u32 length and text, then clock 1 and sent 2, little-endian.
    let bytes = [
        2, 0, 0, 0, b'a', b'b', //
        1, 0, 0, 0, 0, 0, 0, 0, //
        2, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(decode_progress(&bytes), Ok(report("ab", 1, 2)));
}

#[test]
fn a_progress_report_round_trips() {
    let original = report("posmaster", u64::MAX - 1, 42);
    let bytes = encode_progress(&original).expect("encodes");
    assert_eq!(decode_progress(&bytes), Ok(original));
}

#[test]
fn a_truncated_progress_report_is_refused() {
    let bytes = encode_progress(&report("node", 7, 8)).expect("encodes");
    for cut in 0..bytes.len() {
        let truncated = bytes.get(..cut).expect("a prefix");
        assert!(decode_progress(truncated).is_err(), "a prefix of {cut} bytes decoded");
    }
}

#[test]
fn a_progress_report_with_bytes_left_over_is_refused() {
    let mut bytes = encode_progress(&report("node", 7, 8)).expect("encodes");
    bytes.push(0);
    assert!(decode_progress(&bytes).expect_err("trailing byte").contains("follow"));
}

#[test]
fn a_progress_name_that_is_not_text_is_refused() {
    let bytes = [1, 0, 0, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(decode_progress(&bytes).is_err());
}

#[test]
fn the_progress_definition_hashes_to_the_generated_type() {
    let md5 = hypha_node::sdk::md5::md5_from_definition(PROGRESS_DATA_TYPE, PROGRESS_DEFINITION);
    assert_eq!(md5.as_deref(), Some(Progress::SCHEMA_HASH));
    let clock = hypha_node::sdk::msgs::rosgraph_msgs::Clock::TYPE_NAME;
    let md5 = hypha_node::sdk::md5::md5_from_definition(clock, CLOCK_DEFINITION);
    assert_eq!(md5.as_deref(), Some(hypha_node::sdk::msgs::rosgraph_msgs::Clock::SCHEMA_HASH));
}

#[test]
fn pace_reads_a_factor_or_max() {
    assert_eq!(parse_pace("max"), Ok(Pace::Max));
    assert_eq!(parse_pace("2.5"), Ok(Pace::Factor(2.5)));
    assert_eq!(parse_pace("1").expect("one"), Pace::Factor(1.0));
    assert_eq!(Pace::Max.factor(), None);
    assert_eq!(Pace::Factor(4.0).factor(), Some(4.0));
}

#[test]
fn a_pace_shows_its_rate_without_reading_as_zero() {
    assert_eq!(Pace::Factor(4.0).to_string(), "4.00x");
    assert_eq!(Pace::Factor(1e-15).to_string(), "1e-15x");
    assert_eq!(Pace::Max.to_string(), "max");
}

#[test]
fn pace_refuses_zero_negative_and_non_numbers() {
    for text in ["0", "-1", "nan", "inf", "fast", "", "Max"] {
        let error = parse_pace(text).expect_err("refused");
        assert!(error.contains("--rate"), "{error}");
    }
}

#[test]
fn seconds_and_chunks_read_their_units() {
    assert_eq!(parse_seconds("--settle", "2"), Ok(Duration::from_secs(2)));
    assert_eq!(parse_seconds("--settle", "0.25"), Ok(Duration::from_millis(250)));
    assert!(parse_seconds("--timeout", "-1").expect_err("negative").contains("--timeout"));
    assert_eq!(parse_chunk_nanos("100"), Ok(DEFAULT_CHUNK_NANOS));
    assert!(parse_chunk_nanos("0").expect_err("zero").contains("--chunk"));
    assert!(parse_chunk_nanos("1.5").is_err());
}

#[test]
fn the_clock_payload_splits_seconds_and_nanoseconds() {
    let payload = clock_payload(1_500_000_000).expect("in range");
    let mut expected = Vec::new();
    expected.extend_from_slice(&1_u32.to_le_bytes());
    expected.extend_from_slice(&500_000_000_u32.to_le_bytes());
    assert_eq!(payload.to_vec(), expected);
    for nanos in [0_u64, 1, 1_500_000_000, 1_700_000_000_123_456_789] {
        let payload = clock_payload(nanos).expect("in range");
        let decoded = hypha_node::sdk::RosTime::decode_clock(&payload)
            .map(hypha_node::sdk::RosTime::as_nanos);
        assert_eq!(decoded, Some(nanos));
    }
    assert!(clock_payload(u64::MAX).is_err(), "seconds beyond u32 are refused");
}

/// Every boundary that `log_time` closes, in order, as the player takes them.
fn closed_before(plan: &mut ChunkPlan, log_time: u64) -> Vec<u64> {
    let mut closed = Vec::new();
    while let Some(boundary) = plan.next_closed_before(log_time).expect("closes") {
        closed.push(boundary);
    }
    closed
}

#[test]
fn a_chunk_plan_closes_every_boundary_before_the_next_message() {
    let mut plan = ChunkPlan::new(1_000, 100).expect("a plan");
    assert_eq!(closed_before(&mut plan, 1_050), Vec::<u64>::new());
    assert_eq!(closed_before(&mut plan, 1_150), vec![1_100]);
    // A gap closes each of its boundaries, empty or not. A message stamped at 1_400 is
    // held by the chunk that 1_400 closes, so that boundary stays open.
    assert_eq!(closed_before(&mut plan, 1_400), vec![1_200, 1_300]);
    assert_eq!(plan.boundary(), 1_400);
}

#[test]
fn a_message_at_the_boundary_goes_before_its_clock() {
    let mut plan = ChunkPlan::new(1_000, 100).expect("a plan");
    // Messages stamped at the boundary, on one topic or several, all go out first ...
    assert_eq!(closed_before(&mut plan, 1_100), Vec::<u64>::new());
    assert_eq!(closed_before(&mut plan, 1_100), Vec::<u64>::new());
    // ... and the first message after it closes the boundary.
    assert_eq!(closed_before(&mut plan, 1_101), vec![1_100]);
}

#[test]
fn shortening_closes_one_nanosecond_before_the_message() {
    let mut plan = ChunkPlan::new(1_000, 100).expect("a plan");
    assert_eq!(closed_before(&mut plan, 1_050), Vec::<u64>::new());
    assert_eq!(plan.shorten_before(1_050), Some(1_049));
    // The grid is unchanged: the message at 1_100 still goes before the clock 1_100.
    assert_eq!(closed_before(&mut plan, 1_100), Vec::<u64>::new());
    assert_eq!(plan.shorten_before(1_100), Some(1_099));
    assert_eq!(closed_before(&mut plan, 1_101), vec![1_100]);
}

#[test]
fn a_chunk_whose_messages_share_one_instant_cannot_be_shortened() {
    let mut plan = ChunkPlan::new(1_000, 100).expect("a plan");
    assert_eq!(plan.shorten_before(1_050), Some(1_049));
    // Every message of the chunk opened at 1_050 is stamped 1_050.
    assert_eq!(plan.shorten_before(1_050), None);
    assert_eq!(plan.shorten_before(1_051), Some(1_050));
    // After a boundary, the chunk opens one nanosecond past it.
    assert_eq!(closed_before(&mut plan, 1_101), vec![1_100]);
    assert_eq!(plan.shorten_before(1_101), None);
    assert_eq!(plan.shorten_before(1_102), Some(1_101));
}

#[test]
fn the_first_instant_of_a_pass_follows_its_clock() {
    // The start barrier sends `/clock = 1_000` before any data, so the first chunk holds
    // the messages stamped at the start, and none of them can be shortened away.
    let mut plan = ChunkPlan::new(1_000, 100).expect("a plan");
    assert_eq!(closed_before(&mut plan, 1_000), Vec::<u64>::new());
    assert_eq!(plan.shorten_before(1_000), None);
    // Before a message one nanosecond later, the chunk closes at the start again, and then
    // holds the first instant.
    assert_eq!(plan.shorten_before(1_001), Some(1_000));
}

#[test]
fn a_gap_is_taken_one_boundary_at_a_time() {
    // A gap of a million 1 ns chunks is a loop of single steps: nothing is listed up front.
    let mut plan = ChunkPlan::new(0, 1).expect("a plan");
    let mut count = 0_u64;
    while plan.next_closed_before(1_000_000).expect("closes").is_some() {
        count += 1;
    }
    assert_eq!(count, 999_999);
    assert_eq!(plan.boundary(), 1_000_000);
}

#[test]
fn the_final_boundary_stops_at_the_end_of_the_window() {
    let mut plan = ChunkPlan::new(0, 100).expect("a plan");
    assert_eq!(closed_before(&mut plan, 250), vec![100, 200]);
    // The last message is at 250, so the pass ends at the boundary 300 ...
    assert_eq!(plan.end_boundary(None), 300);
    // ... unless the window ends at 260, which is off the chunk grid.
    assert_eq!(plan.end_boundary(Some(260)), 260);
    // A window that ends after the boundary does not move it.
    assert_eq!(plan.end_boundary(Some(400)), 300);
}

#[test]
fn faster_and_slower_never_move_the_rate_the_wrong_way() {
    let same = |a: f64, b: f64| (a - b).abs() < 1e-12;
    assert!(same(faster(1.0), 1.25));
    assert!(same(faster(90.0), MAX_SPEED_FACTOR), "capped at the maximum");
    assert!(same(faster(400.0), 400.0), "a rate above the cap is kept");
    assert!(same(slower(1.0), 0.8));
    assert!(same(slower(0.05), 0.05));
    assert!(same(slower(0.01), 0.01), "a rate below the floor is kept");
}

#[test]
fn a_zero_chunk_is_refused() {
    assert!(ChunkPlan::new(0, 0).is_err());
    assert!(ChunkPlan::new(u64::MAX, 1).is_err());
}

/// What the player sends for `log_times` (sorted, none before `start`): the closes in
/// order, each with how many messages had gone out. A message whose `shorten` flag is set
/// is the one past a full subscriber queue.
fn replay(log_times: &[u64], shorten: &[bool], start: u64, chunk: u64) -> Vec<(u64, usize)> {
    let mut plan = ChunkPlan::new(start, chunk).expect("a plan");
    let mut closes = Vec::new();
    for (published, &time) in log_times.iter().enumerate() {
        closes.extend(closed_before(&mut plan, time).into_iter().map(|close| (close, published)));
        if shorten.get(published).copied().unwrap_or(false)
            && let Some(close) = plan.shorten_before(time)
        {
            closes.push((close, published));
        }
    }
    let last = *log_times.last().expect("a message");
    let published = log_times.len();
    closes.extend(closed_before(&mut plan, last + 1).into_iter().map(|close| (close, published)));
    closes.push((plan.end_boundary(None), published));
    closes
}

proptest::proptest! {
    #[test]
    fn a_replay_keeps_the_rule(
        start in 0_u64..50,
        chunk in 1_u64..20,
        mut offsets in proptest::collection::vec(0_u64..200, 1..40),
        shorten in proptest::collection::vec(proptest::bool::ANY, 40),
    ) {
        offsets.sort_unstable();
        let log_times: Vec<u64> = offsets.iter().map(|offset| start + offset).collect();
        let closes = replay(&log_times, &shorten, start, chunk);
        for &(close, published) in &closes {
            // Every message stamped at or before a close went out before it ...
            let held = log_times.iter().filter(|time| **time <= close).count();
            proptest::prop_assert!(published >= held, "{close} closed before its data");
            // ... and nothing went out past the message the close comes before.
            if let Some(next) = log_times.get(published) {
                proptest::prop_assert!(close < *next, "{close} is not before message {next}");
            }
        }
        // Closes rise; only a first shortened close can repeat the start's clock.
        let values: Vec<u64> = closes.iter().map(|(close, _)| *close).collect();
        proptest::prop_assert!(values.is_sorted_by(|a, b| a < b), "{values:?}");
        proptest::prop_assert!(values.iter().skip(1).all(|close| *close > start), "{values:?}");
        proptest::prop_assert!(values.first().is_some_and(|close| *close >= start));
        // Every boundary of the grid closes, and the pass ends at the first one after the
        // last message.
        let last = *log_times.last().expect("a message");
        let end = *values.last().expect("an end");
        let on_the_grid = |close: u64| close > start && (close - start).is_multiple_of(chunk);
        proptest::prop_assert!(end > last && end - chunk <= last && on_the_grid(end));
        let grid: Vec<u64> = (1..=(end - start) / chunk).map(|k| start + k * chunk).collect();
        let on_grid: Vec<u64> = values.iter().copied().filter(|close| on_the_grid(*close)).collect();
        proptest::prop_assert_eq!(on_grid, grid);
    }
}
