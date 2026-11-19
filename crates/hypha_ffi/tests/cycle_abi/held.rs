//! Cycle C ABI behavior checks.

use super::*;

#[derive(Debug, Clone, Default)]
struct HeldTick {
    held_status: i32,
    held_payload: Vec<u8>,
    held_publisher_id: u64,
    held_publisher_sequence: u64,
    null_view_status: i32,
    unknown_view_status: i32,
    null_image_view_status: i32,
    null_image_report_status: i32,
    report_status: i32,
    report: HyphaCycleHeldReport,
    null_report_status: i32,
    unknown_report_status: i32,
    message_stamp_nanos: i64,
    message_stamp_valid: bool,
}

#[derive(Default)]
struct HeldContext {
    ticks: Mutex<Vec<HeldTick>>,
}

/// Records the held sample, the held report and the stamp of every step call.
extern "C" fn held_recording_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let ctx = unsafe { &*context.cast::<HeldContext>() };
    let mut tick = HeldTick {
        message_stamp_nanos: unsafe { (*time).message_stamp_nanos },
        message_stamp_valid: unsafe { (*time).message_stamp_valid } != 0,
        ..HeldTick::default()
    };

    let mut view = HyphaCycleSampleView {
        payload: ptr::null(),
        payload_len: 0,
        transport_sent_nanos: 0,
        sequence: 0,
        publisher_id: 0,
        publisher_sequence: 0,
    };
    tick.held_status = unsafe { hypha_cycle_image_held(image, 0, &raw mut view) } as i32;
    if tick.held_status == HyphaStatus::Ok as i32 {
        let payload = unsafe { std::slice::from_raw_parts(view.payload, view.payload_len) };
        tick.held_payload = payload.to_vec();
        tick.held_publisher_id = view.publisher_id;
        tick.held_publisher_sequence = view.publisher_sequence;
    }
    tick.null_view_status = unsafe { hypha_cycle_image_held(image, 0, ptr::null_mut()) } as i32;
    tick.unknown_view_status = unsafe { hypha_cycle_image_held(image, 99, &raw mut view) } as i32;
    tick.null_image_view_status =
        unsafe { hypha_cycle_image_held(ptr::null(), 0, &raw mut view) } as i32;

    let mut binding = HyphaCycleBindingView::default();
    tick.report_status = unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) } as i32;
    tick.report = binding.held;
    tick.null_report_status =
        unsafe { hypha_cycle_image_binding(image, 0, ptr::null_mut()) } as i32;
    tick.unknown_report_status =
        unsafe { hypha_cycle_image_binding(image, 99, &raw mut binding) } as i32;
    tick.null_image_report_status =
        unsafe { hypha_cycle_image_binding(ptr::null(), 0, &raw mut binding) } as i32;

    if let Ok(mut ticks) = ctx.ticks.lock() {
        ticks.push(tick);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn recorded_ticks(context: &HeldContext) -> Vec<HeldTick> {
    context.ticks.lock().map_or_else(|_| Vec::new(), |ticks| ticks.clone())
}

unsafe fn create_held_cycle(
    waitset: *mut HyphaWaitSet,
    label: &str,
    context: *mut c_void,
) -> *mut HyphaCycle {
    let name = c_text(label);
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(held_recording_step),
                context,
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    cycle
}

#[test]
fn held_sample_and_report_follow_the_newest_usable_sample() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_held"));
    let waitset = unsafe { create_waitset(node) };
    let in_topic = format!("/{}", unique("held_in"));
    let (pub_in, sub_in) = unsafe { create_pubsub(node, &in_topic) };
    let (pub_out, _sub_out) = unsafe { create_pubsub(node, &format!("/{}", unique("held_out"))) };

    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );

    let context = Box::new(HeldContext::default());
    let cycle = unsafe {
        create_held_cycle(waitset, "held_cycle", (&raw const *context).cast_mut().cast())
    };
    let bind_opts = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut in_idx = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut in_idx) },
        HyphaStatus::Ok
    );
    let mut out_idx = 0;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                pub_out,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut out_idx,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    assert!(run_until(waitset, POLL_BOUND, || !recorded_ticks(&context).is_empty()));
    unsafe { send_sample(pub_in, &[10, 20, 30, 40]) };
    let first_payload_seen = run_until(waitset, POLL_BOUND, || {
        recorded_ticks(&context).iter().any(|tick| tick.held_payload == [10, 20, 30, 40])
    });
    assert!(first_payload_seen, "the first sample never became held");
    unsafe { send_sample(pub_in, &[50]) };
    let second_payload_seen = run_until(waitset, POLL_BOUND, || {
        recorded_ticks(&context).last().is_some_and(|tick| tick.held_payload == [50])
    });
    assert!(second_payload_seen, "the second sample never became held");
    assert!(run_until(waitset, POLL_BOUND, || recorded_ticks(&context).len() >= 3));

    let ticks = recorded_ticks(&context);
    assert!(ticks.len() >= 3, "the cycle stepped {} times", ticks.len());
    let first = ticks.first().expect("first tick");
    assert_eq!(first.held_status, HyphaStatus::NotAvailable as i32);
    assert_eq!(first.report_status, HyphaStatus::Ok as i32);
    assert_eq!(first.report.has_held, 0);
    assert_eq!(first.report.received_total, 0);
    assert!(ticks.iter().any(|tick| tick.held_payload == [10, 20, 30, 40]));

    let last = ticks.last().expect("last tick");
    assert_eq!(last.held_status, HyphaStatus::Ok as i32);
    assert_eq!(last.held_payload, [50]);
    assert_eq!(last.report.has_held, 1);
    assert_eq!(last.report.held_sequence, 1);
    assert_eq!(last.report.received_total, 2);
    assert_eq!(last.report.delivered_total, 2, "a re-selected sample counts once");

    // The view names the frame's publisher and its send sequence: the second sample of
    // one publisher follows the first by one.
    let earlier =
        ticks.iter().find(|tick| tick.held_payload == [10, 20, 30, 40]).expect("first sample");
    assert_eq!(last.held_publisher_id, earlier.held_publisher_id);
    assert_eq!(
        last.held_publisher_sequence,
        earlier.held_publisher_sequence.wrapping_add(1),
        "the publisher sequence of the held sample"
    );

    for tick in &ticks {
        assert_eq!(tick.null_view_status, HyphaStatus::InvalidArgument as i32);
        assert_eq!(tick.unknown_view_status, HyphaStatus::InvalidArgument as i32);
        assert_eq!(tick.null_report_status, HyphaStatus::InvalidArgument as i32);
        assert_eq!(tick.unknown_report_status, HyphaStatus::InvalidArgument as i32);
        assert_eq!(tick.null_image_view_status, HyphaStatus::InvalidArgument as i32);
        assert_eq!(tick.null_image_report_status, HyphaStatus::InvalidArgument as i32);
    }

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
        hypha_node_destroy(node);
    }
}

fn unix_nanos() -> i64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    i64::try_from(since_epoch).unwrap_or(i64::MAX)
}

#[test]
fn message_stamp_is_framework_time_under_sim_time_and_wall_time_otherwise() {
    let context = Box::new(HeldContext::default());
    {
        let sim_node = SimulatedNode::new("cyc_stamp_sim", 1_000_000_000);
        let waitset = unsafe { create_waitset(sim_node.node) };
        let cycle = unsafe {
            create_held_cycle(waitset, "stamp_sim", (&raw const *context).cast_mut().cast())
        };
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        set_sim_clock(1_500_000_000);
        assert!(run_until(waitset, POLL_BOUND, || !recorded_ticks(&context).is_empty()));
        let ticks = recorded_ticks(&context);
        assert!(!ticks.is_empty());
        // A replay step stamps its own deadline, one per 10 ms period, up to the clock value
        // that the run reached.
        let stamps: Vec<i64> = ticks.iter().map(|tick| tick.message_stamp_nanos).collect();
        assert!(stamps.windows(2).all(|pair| pair.first() < pair.last()), "stamps: {stamps:?}");
        assert!(stamps.iter().all(|stamp| stamp % 10_000_000 == 0 && *stamp <= 1_500_000_000));
        assert_eq!(stamps.last().copied(), Some(1_500_000_000));
        assert!(ticks.iter().all(|tick| tick.message_stamp_valid));
        assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    }

    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_stamp_wall"));
    let waitset = unsafe { create_waitset(node) };
    let wall_context = Box::new(HeldContext::default());
    let cycle = unsafe {
        create_held_cycle(waitset, "stamp_wall", (&raw const *wall_context).cast_mut().cast())
    };
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let before = unix_nanos();
    assert!(run_until(waitset, POLL_BOUND, || !recorded_ticks(&wall_context).is_empty()));
    let after = unix_nanos();
    let ticks = recorded_ticks(&wall_context);
    assert!(!ticks.is_empty());
    for tick in &ticks {
        assert!(before <= tick.message_stamp_nanos && tick.message_stamp_nanos <= after);
    }
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}

#[test]
fn a_cycle_does_not_step_at_sim_time_zero_until_sim_time_advances() {
    // The timer runs on framework time, so frozen zero time never makes a tick due.
    let sim_node = SimulatedNode::new("cyc_zero", 0);
    let waitset = unsafe { create_waitset(sim_node.node) };
    let context = Box::new(HeldContext::default());
    let cycle = unsafe {
        create_held_cycle(waitset, "zero_cycle", (&raw const *context).cast_mut().cast())
    };
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    run_waitset_for(waitset, 150_000_000);
    assert!(recorded_ticks(&context).is_empty(), "no step may run at sim time zero");

    set_sim_clock(1_000_000_000);
    let stepped = run_until(waitset, POLL_BOUND, || !recorded_ticks(&context).is_empty());
    assert!(stepped, "the step runs once sim time advances");

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}
