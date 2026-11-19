//! Cycle C ABI behavior checks.

use super::*;

struct TestContext {
    cycle_ptr: *mut HyphaCycle,
    pending_cycle: *mut HyphaCycle,
    waitset: *mut HyphaWaitSet,
    source: *mut HyphaInputSource,
    publisher: *mut HyphaPublisher,
    subscriber: *mut HyphaSubscriber,
    binding_options: HyphaCycleBindingOptions,
    invocations: AtomicU32,
    test_active_destroy_status: AtomicU32,
    saw_sample_payload: AtomicU32,
    active_input_bind_status: AtomicU32,
    active_output_bind_status: AtomicU32,
    active_commit_status: AtomicU32,
    active_pending_destroy_status: AtomicU32,
    active_cycle_create_status: AtomicU32,
    active_source_create_status: AtomicU32,
    active_disabled_source_create_status: AtomicU32,
    invalid_binding_sample_status: AtomicU32,
}

extern "C" fn step_exerciser(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let ctx = unsafe { &*context.cast::<TestContext>() };
    ctx.invocations.fetch_add(1, Ordering::SeqCst);

    let t = unsafe { *time };
    assert!(t.scheduled_nanos >= 0);
    assert!(t.callback_start_nanos >= 0);
    assert!(t.capture_steady_nanos >= 0);

    // The binding view of this step: its capture, held facts and supervision.
    let mut binding = HyphaCycleBindingView::default();
    binding.health.capture.verdict = 99;
    assert_eq!(unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) }, HyphaStatus::Ok);
    let report = binding.health.capture;
    assert_eq!(report.verdict, HYPHA_CYCLE_VERDICT_USABLE);
    assert_eq!(report.sample_count, 1);
    assert_eq!(binding.held.has_held, 1, "the usable sample is the held sample");
    assert_eq!(binding.supervision, HyphaSupervisionReport::default(), "unsupervised");

    // An unknown binding or a null output is refused and writes nothing.
    let mut untouched = HyphaCycleBindingView::default();
    untouched.health.capture.verdict = 99;
    assert_eq!(
        unsafe { hypha_cycle_image_binding(image, 99, &raw mut untouched) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(untouched.health.capture.verdict, 99);
    assert_eq!(
        unsafe { hypha_cycle_image_binding(image, 0, ptr::null_mut()) },
        HyphaStatus::InvalidArgument
    );

    let mut sample_view = HyphaCycleSampleView {
        payload: ptr::null(),
        payload_len: 0,
        transport_sent_nanos: 0,
        sequence: 0,
        publisher_id: 0,
        publisher_sequence: 0,
    };
    assert_eq!(
        unsafe { hypha_cycle_image_sample(image, 0, 0, &raw mut sample_view) },
        HyphaStatus::Ok
    );
    assert_eq!(sample_view.payload_len, 4);
    assert!(!sample_view.payload.is_null());
    let slice = unsafe { std::slice::from_raw_parts(sample_view.payload, sample_view.payload_len) };
    if slice == [10, 20, 30, 40] {
        ctx.saw_sample_payload.store(1, Ordering::SeqCst);
    }
    // Out-of-bounds sample index returns NotAvailable
    assert_eq!(
        unsafe { hypha_cycle_image_sample(image, 0, 99, &raw mut sample_view) },
        HyphaStatus::NotAvailable
    );
    let invalid_binding_sample_status =
        unsafe { hypha_cycle_image_sample(image, 99, 0, &raw mut sample_view) };
    ctx.invalid_binding_sample_status.store(invalid_binding_sample_status as u32, Ordering::SeqCst);

    let out_bytes = [100, 101];
    assert_eq!(unsafe { emit_message(emission, 0, &out_bytes) }, HyphaStatus::Ok);

    let destroy_status = unsafe { hypha_cycle_destroy(ctx.cycle_ptr) };
    ctx.test_active_destroy_status.store(destroy_status as u32, Ordering::SeqCst);

    let mut input_index = 99;
    let input_status = unsafe {
        hypha_cycle_bind_input(
            ctx.pending_cycle,
            ctx.source,
            &raw const ctx.binding_options,
            &raw mut input_index,
        )
    };
    ctx.active_input_bind_status.store(input_status as u32, Ordering::SeqCst);
    let mut output_index = 99;
    let output_status = unsafe {
        hypha_cycle_bind_output(
            ctx.pending_cycle,
            ctx.publisher,
            HyphaCycleOutputFailure::FailRun as u32,
            1,
            &raw mut output_index,
        )
    };
    ctx.active_output_bind_status.store(output_status as u32, Ordering::SeqCst);
    let commit_status = unsafe { hypha_cycle_commit(ctx.pending_cycle) };
    ctx.active_commit_status.store(commit_status as u32, Ordering::SeqCst);
    let pending_destroy_status = unsafe { hypha_cycle_destroy(ctx.pending_cycle) };
    ctx.active_pending_destroy_status.store(pending_destroy_status as u32, Ordering::SeqCst);

    let pending_name = c"active_cycle_create";
    let mut created_cycle = ptr::null_mut();
    let create_status = unsafe {
        hypha_cycle_create(
            ctx.waitset,
            pending_name.as_ptr(),
            pending_name.to_bytes().len(),
            10_000_000,
            Some(empty_step),
            ptr::null_mut(),
            None,
            &raw mut created_cycle,
        )
    };
    ctx.active_cycle_create_status.store(create_status as u32, Ordering::SeqCst);

    let mut created_source = ptr::null_mut();
    let source_create_status =
        unsafe { hypha_input_source_create(ctx.waitset, ctx.subscriber, &raw mut created_source) };
    ctx.active_source_create_status.store(source_create_status as u32, Ordering::SeqCst);
    let mut created_disabled_source = ptr::null_mut();
    let disabled_create_status = unsafe {
        hypha_input_source_create(ctx.waitset, ptr::null_mut(), &raw mut created_disabled_source)
    };
    ctx.active_disabled_source_create_status.store(disabled_create_status as u32, Ordering::SeqCst);

    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn cycle_execution_image_emission_and_health_abi() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_exec"));
    let waitset = unsafe { create_waitset(node) };

    let in_topic = format!("/{}", unique("e_in"));
    let out_topic = format!("/{}", unique("e_out"));
    let (pub_in, sub_in) = unsafe { create_pubsub(node, &in_topic) };
    let (pub_out, _sub_out) = unsafe { create_pubsub(node, &out_topic) };
    let (pending_output, pending_receiver) =
        unsafe { create_pubsub(node, &format!("/{}", unique("pending"))) };

    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );

    let mut context_box = Box::new(TestContext {
        cycle_ptr: ptr::null_mut(),
        pending_cycle: ptr::null_mut(),
        waitset,
        source: ptr::null_mut(),
        publisher: ptr::null_mut(),
        subscriber: ptr::null_mut(),
        binding_options: HyphaCycleBindingOptions {
            mode: HyphaCycleInputMode::Latest as u32,
            max_samples_per_iteration: 1,
            optional: 0,
        },
        invocations: AtomicU32::new(0),
        test_active_destroy_status: AtomicU32::new(0),
        saw_sample_payload: AtomicU32::new(0),
        active_input_bind_status: AtomicU32::new(0),
        active_output_bind_status: AtomicU32::new(0),
        active_commit_status: AtomicU32::new(0),
        active_pending_destroy_status: AtomicU32::new(0),
        active_cycle_create_status: AtomicU32::new(0),
        active_source_create_status: AtomicU32::new(0),
        active_disabled_source_create_status: AtomicU32::new(0),
        invalid_binding_sample_status: AtomicU32::new(0),
    });

    let name = c_text("exec_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000, // 10ms
                Some(step_exerciser),
                (&raw mut *context_box).cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    context_box.cycle_ptr = cycle;

    let pending_name = c_text("pending_cycle");
    let mut pending_cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                pending_name.as_ptr(),
                pending_name.as_bytes().len(),
                10_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut pending_cycle,
            )
        },
        HyphaStatus::Ok
    );
    context_box.pending_cycle = pending_cycle;
    context_box.source = source;
    context_box.publisher = pending_output;
    context_box.subscriber = pending_receiver;

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

    unsafe { send_sample(pub_in, &[10, 20, 30, 40]) };

    assert!(run_until(waitset, POLL_BOUND, || context_box.invocations.load(Ordering::SeqCst) >= 1));

    assert!(context_box.invocations.load(Ordering::SeqCst) >= 1);
    assert_eq!(context_box.saw_sample_payload.load(Ordering::SeqCst), 1);
    // Active run destroy was rejected with InvalidState
    assert_eq!(
        context_box.test_active_destroy_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.active_input_bind_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.active_output_bind_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.active_commit_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.active_pending_destroy_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.active_cycle_create_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.active_source_create_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.active_disabled_source_create_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidState as u32
    );
    assert_eq!(
        context_box.invalid_binding_sample_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidArgument as u32
    );

    // Refused operations leave their caller-owned handles available.
    let mut input_index = 99;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_input(
                pending_cycle,
                source,
                &raw const context_box.binding_options,
                &raw mut input_index,
            )
        },
        HyphaStatus::Ok
    );
    let mut output_index = 99;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                pending_cycle,
                pending_output,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut output_index,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(pending_cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(pending_cycle) }, HyphaStatus::Ok);
    let mut recovered_source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, pending_receiver, &raw mut recovered_source) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_input_source_destroy(recovered_source) }, HyphaStatus::Ok);
    let mut health = HyphaCycleHealth::default();
    assert_eq!(unsafe { hypha_cycle_health(cycle, &raw mut health) }, HyphaStatus::Ok);
    assert!(health.iterations >= 1);

    // The binding view of the last finished step, read between steps.
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    assert_eq!(binding.health.capture.verdict, HYPHA_CYCLE_VERDICT_USABLE);
    assert_eq!(
        unsafe { hypha_cycle_binding(cycle, 99, &raw mut binding) },
        HyphaStatus::InvalidArgument
    );

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
        hypha_node_destroy(node);
    }
}

#[test]
fn a_rewind_of_simulated_time_ends_the_run() {
    let sim_node = SimulatedNode::new("cyc_rewind", 1_000_000_000);
    let waitset = unsafe { create_waitset(sim_node.node) };
    let name = c_text("rewind_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                50_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    set_sim_clock(1_100_000_000);
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 150_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    assert_eq!(report.status, HyphaStatus::Ok as i32, "time moved forward");

    set_sim_clock(500_000_000);
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 150_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    assert_eq!(report.reason, HYPHA_RUN_REASON_RUNTIME_FAILED);
    assert_eq!(report.status, HyphaStatus::ClockRewound as i32);

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}
