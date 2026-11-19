//! The image report of input supervision: silence, stagnation and its log lines.

use super::*;

/// The cycle's own report of binding `index`, read between steps.
fn assert_binding_report(cycle: *mut HyphaCycle, index: u32, faults: u32, ignored: u8) {
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, index, &raw mut binding) }, HyphaStatus::Ok);
    let report = binding.supervision;
    assert_eq!((report.faults, report.ignored), (faults, ignored));
}

#[test]
fn image_supervision_reports_the_current_iteration() {
    let parameters = port_parameters("sup_report", SPEED_TABLE);
    let sim = SimulatedNode::with_parameters("sup_report", 1_000_000_000, Some(&parameters));
    let waitset = unsafe { create_waitset(sim.node) };
    let (pub_in, sub_in) = unsafe { create_pubsub(sim.node, &format!("/{}", unique("report_in"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );

    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "report_cycle", (&raw const *context).cast_mut().cast())
    };
    let bind_opts = bind_options();
    let mut in_idx = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut in_idx) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { bind(cycle, sim.node, 0, "speed", BOTH_CHECKS).status }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    assert!(
        crate::common::introspection::introspection_text(sim.node)
            .contains("silence=100000000,stagnation=300000000,counted")
    );
    let mut driver = SimDriver { waitset, now: 1_000_000_000 };

    // Staleness: no sample has arrived, so the report turns stale past the limit.
    let silent = slices_until(
        &mut driver,
        &context,
        || {},
        |ticks| ticks.last().is_some_and(|tick| tick.report.faults & HYPHA_FAULT_STALE != 0),
    );
    assert!(silent, "the input never reported silence");
    // Between steps the cycle answers for its binding with the same report.
    assert_binding_report(cycle, 0, HYPHA_FAULT_STALE, 0);
    let ticks = recorded_ticks(&context);
    let first = ticks.first().expect("first tick");
    let silent_at = ticks
        .iter()
        .position(|tick| tick.report.faults & HYPHA_FAULT_STALE != 0)
        .expect("silent tick");
    let silent_tick = ticks[silent_at];
    let before_silent = ticks[silent_at - 1];
    assert!(silent_tick.callback_start_nanos - first.callback_start_nanos > SILENCE_NANOS);
    assert!(before_silent.callback_start_nanos - first.callback_start_nanos <= SILENCE_NANOS);

    // A sample clears silence in the same iteration that accepts it.
    unsafe { send_sample(pub_in, &[1]) };
    let accepted = slices_until(
        &mut driver,
        &context,
        || {},
        |ticks| ticks.iter().any(|tick| tick.received_total > 0),
    );
    assert!(accepted, "the sample was never accepted");
    let ticks = recorded_ticks(&context);
    let accepted_at = ticks.iter().position(|tick| tick.received_total > 0).expect("accepted tick");
    assert_eq!(ticks[accepted_at].report.faults & HYPHA_FAULT_STALE, 0, "accepting clears silence");
    assert_ne!(
        ticks[accepted_at - 1].report.faults & HYPHA_FAULT_STALE,
        0,
        "the tick before was silent"
    );

    // Stagnation: samples keep arriving, but the alive counter does not change.
    let alive_from = ticks.last().map_or(0, |tick| tick.callback_start_nanos);
    context.alive_value.store(7, Ordering::SeqCst);
    context.publish_alive.store(true, Ordering::SeqCst);
    let stagnant = slices_until(
        &mut driver,
        &context,
        || unsafe { send_sample(pub_in, &[2]) },
        |ticks| ticks.last().is_some_and(|tick| tick.report.faults & HYPHA_FAULT_STAGNANT != 0),
    );
    assert!(stagnant, "the unchanged alive counter never reported stagnation");
    let last = recorded_ticks(&context).last().copied().expect("stagnant tick");
    assert_eq!(last.report.faults & HYPHA_FAULT_STALE, 0, "samples keep the input from silence");
    assert!(last.callback_start_nanos - alive_from >= STAGNATION_NANOS);

    // Ignoring sets the flag and keeps the fault bits.
    context.ignore.store(true, Ordering::SeqCst);
    let ignored = slices_until(
        &mut driver,
        &context,
        || unsafe { send_sample(pub_in, &[3]) },
        |ticks| ticks.last().is_some_and(|tick| tick.report.ignored == 1),
    );
    assert!(ignored, "the ignore flag never reached the report");
    assert!(
        crate::common::introspection::introspection_text(sim.node)
            .contains("silence=100000000,stagnation=300000000,ignored")
    );
    let last = recorded_ticks(&context).last().copied().expect("ignored tick");
    assert_ne!(last.report.faults & HYPHA_FAULT_STAGNANT, 0, "ignoring keeps the fault bits");
    assert_eq!(last.report.ignored, 1);
    assert_binding_report(cycle, 0, last.report.faults, 1);

    // The flag was consumed in one step, so the override must come from the runtime.
    for _ in 0..3 {
        unsafe { send_sample(pub_in, &[4]) };
        driver.slice();
    }
    let later = recorded_ticks(&context).last().copied().expect("later tick");
    assert_eq!(later.report.ignored, 1, "the runtime keeps the override after the step");
    assert_ne!(later.report.faults & HYPHA_FAULT_STAGNANT, 0);

    context.restore.store(true, Ordering::SeqCst);
    assert!(slices_until(
        &mut driver,
        &context,
        || unsafe { send_sample(pub_in, &[5]) },
        |ticks| ticks.last().is_some_and(|tick| tick.report.ignored == 0),
    ));
    assert!(
        crate::common::introspection::introspection_text(sim.node)
            .contains("silence=100000000,stagnation=300000000,counted")
    );
    assert_binding_report(
        cycle,
        0,
        recorded_ticks(&context).last().map_or(0, |t| t.report.faults),
        0,
    );

    for tick in recorded_ticks(&context) {
        assert_eq!(tick.report_status, HyphaStatus::Ok as i32);
        assert_refused_image_calls(&tick);
    }

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
    }
}

#[test]
fn stagnation_waits_for_its_limit_and_restarts_on_a_changed_alive_value() {
    let parameters = port_parameters("sup_restart", "silence_ms = 1000\nstagnation_ms = 300");
    let sim = SimulatedNode::with_parameters("sup_restart", 1_000_000_000, Some(&parameters));
    let waitset = unsafe { create_waitset(sim.node) };
    let (pub_in, sub_in) =
        unsafe { create_pubsub(sim.node, &format!("/{}", unique("restart_in"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );

    let context = Box::new(Context::default());
    let cycle = unsafe {
        create_supervised_cycle(waitset, "restart_cycle", (&raw const *context).cast_mut().cast())
    };
    let bind_opts = bind_options();
    let mut in_idx = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut in_idx) },
        HyphaStatus::Ok
    );
    // Silence stays off the table: a sample reaches every slice.
    assert_eq!(unsafe { bind(cycle, sim.node, 0, "speed", BOTH_CHECKS).status }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let mut driver = SimDriver { waitset, now: 1_000_000_000 };
    context.alive_value.store(7, Ordering::SeqCst);
    context.publish_alive.store(true, Ordering::SeqCst);

    // The first tick after the flag sets alive 7. The stagnation limit counts from it.
    let stagnant = slices_until(
        &mut driver,
        &context,
        || unsafe { send_sample(pub_in, &[5]) },
        |ticks| ticks.last().is_some_and(|tick| tick.report.faults & HYPHA_FAULT_STAGNANT != 0),
    );
    assert!(stagnant, "the unchanged alive counter never reported stagnation");
    let ticks = recorded_ticks(&context);
    let alive_cb = ticks.first().expect("first tick").callback_start_nanos;
    let stagnant_at = ticks
        .iter()
        .position(|tick| tick.report.faults & HYPHA_FAULT_STAGNANT != 0)
        .expect("stagnant");
    let stagnant_tick = ticks[stagnant_at];
    let before_limit = ticks[stagnant_at - 1];
    assert!(stagnant_tick.callback_start_nanos - alive_cb >= STAGNATION_NANOS);
    assert_eq!(before_limit.report.faults & HYPHA_FAULT_STAGNANT, 0, "one tick before the limit");
    assert!(before_limit.callback_start_nanos - alive_cb < STAGNATION_NANOS);

    // A changed alive value restarts the timer at once.
    let change_from = ticks.len();
    context.alive_value.store(8, Ordering::SeqCst);
    let moved = slices_until(
        &mut driver,
        &context,
        || unsafe { send_sample(pub_in, &[6]) },
        |ticks| ticks.len() > change_from,
    );
    assert!(moved, "the changed alive value was never read");
    let ticks = recorded_ticks(&context);
    let changed = ticks[change_from];
    assert_eq!(
        changed.report.faults & HYPHA_FAULT_STAGNANT,
        0,
        "a changed value clears stagnation"
    );

    let restagnant = slices_until(
        &mut driver,
        &context,
        || unsafe { send_sample(pub_in, &[7]) },
        |ticks| ticks.last().is_some_and(|tick| tick.report.faults & HYPHA_FAULT_STAGNANT != 0),
    );
    assert!(restagnant, "the new alive value never went stagnant");
    let last = recorded_ticks(&context).last().copied().expect("restagnant tick");
    assert!(last.callback_start_nanos - changed.callback_start_nanos >= STAGNATION_NANOS);

    for tick in recorded_ticks(&context) {
        assert_refused_image_calls(&tick);
    }

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
    }
}

fn log_count(text: &str, level: &str, message: &str) -> usize {
    let level_tag = format!("[{level}]");
    text.lines()
        .filter(|line| line.contains(&level_tag) && line.trim_end().ends_with(message))
        .count()
}

/// One supervised input through every fault transition. The log holds one line each.
/// The test owns its process, so its `[log]` table is the first one of the process.
#[test]
fn the_log_records_each_supervision_transition_once() {
    let dir = std::env::temp_dir().join(unique("log"));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let params = dir.join("params.toml");
    let log_table = "[log]\nconsole = false\n[log.sink.main]\npath = \"supervision.log\"\nlevel = \"info\"\n\n[ports.inputs.speed]\ntopic = \"/speed\"\nsilence_ms = 100\n";
    std::fs::write(&params, log_table).expect("write parameter fixture");

    let sim = SimulatedNode::with_parameters("sup_log", 1_000_000_000, Some(&params));
    let waitset = unsafe { create_waitset(sim.node) };
    let topic = format!("/{}", unique("log_in"));
    let (pub_in, sub_in) = unsafe { create_pubsub(sim.node, &topic) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );
    let context = Box::new(Context::default());
    let cycle_name = "log_cycle";
    let cycle = unsafe {
        create_supervised_cycle(waitset, cycle_name, (&raw const *context).cast_mut().cast())
    };
    let bind_opts = bind_options();
    let mut in_idx = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut in_idx) },
        HyphaStatus::Ok
    );
    assert_eq!(
        unsafe { bind(cycle, sim.node, 0, "speed", (true, false, true)).status },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let mut driver = SimDriver { waitset, now: 1_000_000_000 };
    let silent = |ticks: &[Tick]| {
        ticks.last().is_some_and(|tick| tick.report.faults & HYPHA_FAULT_STALE != 0)
    };
    let ignored = |ticks: &[Tick]| ticks.last().is_some_and(|tick| tick.report.ignored != 0);
    let recovered = |ticks: &[Tick]| ticks.last().is_some_and(|tick| tick.report.faults == 0);
    let send = || unsafe { send_sample(pub_in, &[1]) };

    // Silent, not ignored.
    assert!(slices_until(&mut driver, &context, || {}, silent), "silence never started");
    // Ignored while faulted.
    context.ignore.store(true, Ordering::SeqCst);
    assert!(slices_until(&mut driver, &context, || {}, ignored), "override never applied");
    // Recovered while ignored.
    assert!(slices_until(&mut driver, &context, send, recovered), "sample never recovered");
    // Silent again, already ignored.
    assert!(slices_until(&mut driver, &context, || {}, silent), "silence never restarted");
    // Override removed while faulted.
    context.restore.store(true, Ordering::SeqCst);
    assert!(
        slices_until(&mut driver, &context, || {}, |ticks| !ignored(ticks)),
        "override never removed"
    );
    // Recovered, not ignored.
    assert!(slices_until(&mut driver, &context, send, recovered), "sample never recovered");

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
    }
    drop(sim);

    let text = std::fs::read_to_string(dir.join("supervision.log")).expect("read sink file");
    let subject = format!("cycle '{cycle_name}' input 'speed' ({topic})");
    // The lines that carry an age match by their start; the others whole.
    let starts = [
        ("WARN", format!("hypha: {subject} never arrived: no sample for ")),
        ("WARN", format!("hypha: {subject} is stale: newest sample is ")),
    ];
    for (level, start) in starts {
        let level_tag = format!("[{level}]");
        let count = text
            .lines()
            .filter(|line| line.contains(&level_tag) && line.contains(&start))
            .filter(|line| {
                line.trim_end().ends_with(" ms old, silence_ms = 100")
                    || line.trim_end().ends_with(" ms, silence_ms = 100")
            })
            .count();
        assert_eq!(count, 1, "`{start}` count in log: {text}");
    }
    let expected = [
        ("WARN", format!("hypha: {subject} fault is ignored")),
        ("INFO", format!("hypha: {subject} is usable")),
        ("WARN", format!("hypha: {subject} fault counts again")),
        ("INFO", format!("hypha: {subject} is usable again")),
    ];
    for (level, message) in expected {
        let count = log_count(&text, level, &message);
        assert_eq!(count, 1, "`{message}` count in log: {text}");
    }
    // Every supervision line names the subject. No other line may.
    let supervision_lines = text.lines().filter(|line| line.contains(&subject)).count();
    assert_eq!(supervision_lines, 6, "supervision lines in log: {text}");
}
