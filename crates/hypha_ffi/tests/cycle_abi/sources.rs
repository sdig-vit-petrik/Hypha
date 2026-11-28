//! Cycle C ABI behavior checks.

use super::*;

#[test]
fn input_source_lifecycle_and_validation() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("src_test"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) = unsafe { create_pubsub(node, &format!("/{}", unique("s1"))) };

    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(ptr::null_mut(), subscriber, &raw mut source) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, ptr::null_mut()) },
        HyphaStatus::InvalidArgument
    );

    // Takes ownership of the subscriber.
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    assert!(!source.is_null());

    let mut disabled_source = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_input_source_create(ptr::null_mut(), ptr::null_mut(), &raw mut disabled_source)
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, ptr::null_mut(), ptr::null_mut()) },
        HyphaStatus::InvalidArgument
    );

    assert_eq!(
        unsafe { hypha_input_source_create(waitset, ptr::null_mut(), &raw mut disabled_source) },
        HyphaStatus::Ok
    );
    assert!(!disabled_source.is_null());

    assert_eq!(
        unsafe { hypha_input_source_destroy(ptr::null_mut()) },
        HyphaStatus::InvalidArgument
    );

    assert_eq!(unsafe { hypha_input_source_destroy(disabled_source) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);

    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

#[test]
fn disabled_source_can_be_destroyed_after_waitset() {
    let node = create_node(&unique("disabled_source_lifetime"));
    let waitset = unsafe { create_waitset(node) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, ptr::null_mut(), &raw mut source) },
        HyphaStatus::Ok
    );

    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_source_that_outlives_its_waitset_is_destroyed_as_not_running() {
    let node = create_node(&unique("source_outlives_waitset"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let mut source = ptr::null_mut();
    // SAFETY: `waitset` is live; a null subscriber makes an unwired source; `source` is a local.
    let created = unsafe { hypha_input_source_create(waitset, ptr::null_mut(), &raw mut source) };
    assert_eq!(created, HyphaStatus::Ok);

    // The node's destruction destroys its `WaitSet`; the caller's source handle stays.
    // SAFETY: `node` is live and destroyed once; the source handle is not part of it.
    unsafe { hypha_node_destroy(node) };
    // SAFETY: `source` is the live handle created above, destroyed once.
    let destroyed = unsafe { hypha_input_source_destroy(source) };
    assert_eq!(
        destroyed,
        HyphaStatus::Ok,
        "a destroyed WaitSet runs nothing, so the destroy is not refused"
    );
}

#[test]
/// The history is bounded by samples, not bytes.
fn a_large_frame_reaches_a_cycle_input() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("source_history_health"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) =
        unsafe { create_pubsub_carrying(node, &format!("/{}", unique("history_health")), 1 << 20) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );

    let name = c_text("history_health_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                5_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let binding = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut index = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const binding, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    unsafe {
        send_sample(publisher, &vec![1u8; 1 << 20]);
    }
    let mut run_report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 50_000_000, &raw mut run_report) },
        HyphaStatus::Ok
    );
    assert_eq!(run_report.status, HyphaStatus::Ok as i32);
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, index, &raw mut binding) }, HyphaStatus::Ok);
    let health = binding.health;
    assert_eq!(health.capture.verdict, HYPHA_CYCLE_VERDICT_USABLE);
    assert_eq!(health.capture.sample_count, 1);
    assert_eq!(health.overrun_total, 0);
    assert_eq!(health.capture.lost_total, 0);

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

/// A host cycle with one binding of `mode` on a source that received `values` before
/// one run; returns the binding view after the run.
fn binding_after_burst(mode: HyphaCycleInputMode, values: u8) -> HyphaCycleBindingView {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("source_burst"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) =
        unsafe { create_pubsub_carrying(node, &format!("/{}", unique("burst")), 64) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let name = c_text("burst_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                5_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let binding = HyphaCycleBindingOptions {
        mode: mode as u32,
        max_samples_per_iteration: usize::from(values),
        optional: 0,
    };
    let mut index = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const binding, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    for value in 1..=values {
        unsafe { send_sample(publisher, &[value]) };
    }
    // Run until a step has captured the whole burst, not for a fixed slice: how soon a
    // loaded host gives the 5 ms cycle its first step is the host's speed. The bound only
    // ends a run that never captures.
    let mut view = HyphaCycleBindingView::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let mut run_report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        assert_eq!(
            unsafe { hypha_waitset_run(waitset, 10_000_000, &raw mut run_report) },
            HyphaStatus::Ok
        );
        assert_eq!(run_report.status, HyphaStatus::Ok as i32);
        assert_eq!(unsafe { hypha_cycle_binding(cycle, index, &raw mut view) }, HyphaStatus::Ok);
        if view.held.received_total >= u64::from(values) || std::time::Instant::now() > deadline {
            break;
        }
    }
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    view
}

#[test]
fn a_latest_source_in_host_mode_keeps_the_newest_sample_and_counts_every_one() {
    // Four samples: the helper's subscriber queue holds four.
    let view = binding_after_burst(HyphaCycleInputMode::Latest, 4);
    assert_eq!(view.health.capture.verdict, HYPHA_CYCLE_VERDICT_USABLE);
    assert_eq!(view.held.received_total, 4, "every sample counts as accepted");
    assert_ne!(view.held.has_held, 0);
    assert_eq!(view.held.held_sequence, 3, "the held sample is the fourth");
    assert_eq!(view.health.overrun_total, 0);
}

#[test]
fn an_arrivals_binding_on_a_source_keeps_every_sample() {
    let view = binding_after_burst(HyphaCycleInputMode::Queued, 4);
    assert_eq!(view.held.received_total, 4);
    assert_eq!(view.held.delivered_total, 4, "the steps saw every sample");
}

extern "C" fn ignore_sample(
    _delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_subscriber_attached_to_a_waitset_cannot_become_an_input_source() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("source_claim_attached"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("claim_attached"))) };
    let record =
        crate::common::attach::subscription(subscriber, Some(ignore_sample), ptr::null_mut(), None);
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::InvalidOperation
    );
    assert!(source.is_null());
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

#[test]
fn a_source_subscriber_cannot_be_attached_or_sourced_twice() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("source_claim_owned"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("claim_owned"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    // The source holds the subscriber alive; the old handle still names it.
    let record =
        crate::common::attach::subscription(subscriber, Some(ignore_sample), ptr::null_mut(), None);
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::InvalidOperation
    );
    let mut second = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut second) },
        HyphaStatus::InvalidOperation,
        "one source per subscriber"
    );
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}

#[test]
fn the_binding_view_carries_the_age_the_judgement_used() {
    // The run lasts 20 ms of host time; the samples were sent just before it. The bound
    // is loose for a loaded machine.
    let run_bound_nanos = 2_000_000_000;
    let arrived = binding_after_burst(HyphaCycleInputMode::Latest, 1);
    assert_eq!(arrived.health.capture.verdict, HYPHA_CYCLE_VERDICT_USABLE);
    assert_eq!(arrived.ok, 1, "a usable required input without supervision is ok");
    assert!(arrived.age_nanos > 0, "the newest sample has aged");
    assert!(arrived.age_nanos < run_bound_nanos, "age {}", arrived.age_nanos);

    let never = binding_after_burst(HyphaCycleInputMode::Latest, 0);
    assert_eq!(never.health.capture.verdict, HYPHA_CYCLE_VERDICT_NEVER_ARRIVED);
    assert_eq!(never.ok, 0, "a required input that never arrived is not ok");
    assert!(never.age_nanos < run_bound_nanos, "age since the first capture {}", never.age_nanos);
}

#[test]
fn a_cycle_refuses_a_source_of_another_nodes_waitset() {
    let node = create_node(&unique("foreign_source_cycle"));
    let other_node = create_node(&unique("foreign_source_owner"));
    // SAFETY: both nodes are live nodes just created.
    let (waitset, other) = unsafe { (create_waitset(node), create_waitset(other_node)) };
    // SAFETY: `other_node` is live.
    let (publisher, subscriber) =
        unsafe { create_pubsub(other_node, &format!("/{}", unique("foreign_source"))) };
    let mut source = ptr::null_mut();
    // SAFETY: the second node's `WaitSet` and subscriber are live; `source` is a local.
    let created = unsafe { hypha_input_source_create(other, subscriber, &raw mut source) };
    assert_eq!(created, HyphaStatus::Ok);
    let name = c_text("foreign_source");
    let mut cycle = ptr::null_mut();
    // SAFETY: `waitset` is live; the name comes with its length; `cycle` is a local.
    let status = unsafe {
        hypha_cycle_create(
            waitset,
            name.as_ptr(),
            name.as_bytes().len(),
            10_000_000,
            Some(empty_step),
            ptr::null_mut(),
            None,
            &raw mut cycle,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    let options = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 1,
    };
    let mut index = 999;
    // SAFETY: the cycle and the source are live; `options` and `index` are locals.
    let bound =
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) };
    assert_eq!(bound, HyphaStatus::InvalidArgument, "a source of another WaitSet is refused");
    assert_eq!(index, 0, "a refused bind writes the cleared index");

    // SAFETY: every handle is live and destroyed once, the nodes last.
    unsafe {
        assert_eq!(hypha_input_source_destroy(source), HyphaStatus::Ok);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
        hypha_node_destroy(other_node);
    }
}
