//! Cycle C ABI behavior checks.

use super::*;

extern "C" fn latching_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    // Invalid index returns InvalidArgument and latches.
    assert_eq!(unsafe { emit_message(emission, 99, &[1]) }, HyphaStatus::InvalidArgument);
    // Latched error returned on subsequent reserve.
    assert_eq!(unsafe { emit_message(emission, 0, &[1]) }, HyphaStatus::InvalidArgument);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn cycle_emission_error_latching_abi() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_latch"));
    let waitset = unsafe { create_waitset(node) };
    let (pub_out, _sub_out) = unsafe { create_pubsub(node, &format!("/{}", unique("l_out"))) };

    let name = c_text("latch_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(latching_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
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

    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    // The latched emission error fails the run iteration. Poll until it does.
    let started = std::time::Instant::now();
    while report.status != HyphaStatus::InvalidArgument as i32 && started.elapsed() < POLL_BOUND {
        let _ = unsafe { hypha_waitset_run(waitset, 10_000_000, &raw mut report) };
    }
    assert_eq!(report.status, HyphaStatus::InvalidArgument as i32);

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}

struct NullOutContext {
    first_status: AtomicU32,
    second_status: AtomicU32,
}

extern "C" fn null_out_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let state = unsafe { &*context.cast::<NullOutContext>() };
    // A null out pointer is a caller error. It is refused and not latched.
    let first = unsafe { hypha_cycle_emission_reserve(emission, 0, 1, ptr::null_mut()) };
    state.first_status.store(first as u32, Ordering::SeqCst);
    let second = unsafe { emit_message(emission, 0, &[9]) };
    state.second_status.store(second as u32, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn null_out_pointer_is_refused_without_latching_abi() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_null_out"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("null_out_out"))) };
    let name = c_text("null_out_cycle");
    let mut callback_context =
        NullOutContext { first_status: AtomicU32::new(0), second_status: AtomicU32::new(0) };
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                1_000_000,
                Some(null_out_step),
                (&raw mut callback_context).cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let mut output_index = 0;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                publisher,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut output_index,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, 20_000_000, &raw mut report) }, HyphaStatus::Ok);
    assert_eq!(
        callback_context.first_status.load(Ordering::SeqCst),
        HyphaStatus::InvalidArgument as u32
    );
    assert_eq!(callback_context.second_status.load(Ordering::SeqCst), HyphaStatus::Ok as u32);
    assert_eq!(report.status, HyphaStatus::Ok as i32);
    let mut health = HyphaCycleHealth::default();
    assert_eq!(unsafe { hypha_cycle_health(cycle, &raw mut health) }, HyphaStatus::Ok);
    // The step runs once per tick. Every tick sends its one good message.
    assert!(health.iterations >= 1);
    assert_eq!(health.output_attempts, health.iterations);
    assert_eq!(health.output_successes, health.output_attempts);

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_node_destroy(node);
    }
}

/// Which protocol mistake a step makes around a reservation.
#[derive(Clone, Copy)]
enum ProtocolMistake {
    /// Commit with no reservation open.
    CommitWithoutReserve,
    /// Reserve a second message before committing the first.
    ReserveWhileOpen,
    /// Return from the step with a reservation still open.
    ReturnWhileOpen,
}

struct ProtocolContext {
    mistake: ProtocolMistake,
}

extern "C" fn protocol_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let state = unsafe { &*context.cast::<ProtocolContext>() };
    match state.mistake {
        // The protocol error is latched, and the run reports it.
        ProtocolMistake::CommitWithoutReserve => {
            let _ = unsafe { hypha_cycle_emission_commit(emission) };
        }
        ProtocolMistake::ReserveWhileOpen | ProtocolMistake::ReturnWhileOpen => {
            let mut payload = ptr::null_mut();
            let first = unsafe { hypha_cycle_emission_reserve(emission, 0, 1, &raw mut payload) };
            assert_eq!(first, HyphaStatus::Ok);
            if matches!(state.mistake, ProtocolMistake::ReserveWhileOpen) {
                let _ = unsafe { hypha_cycle_emission_reserve(emission, 0, 1, &raw mut payload) };
            }
        }
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn run_protocol_mistake(label: &str, mistake: ProtocolMistake) -> (i32, u64) {
    let node = create_node(&unique(label));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, _subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("protocol_out"))) };
    let name = c_text(label);
    let mut callback_context = ProtocolContext { mistake };
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                1_000_000,
                Some(protocol_step),
                (&raw mut callback_context).cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let mut output_index = 0;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                publisher,
                HyphaCycleOutputFailure::FailRun as u32,
                2,
                &raw mut output_index,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, 20_000_000, &raw mut report) }, HyphaStatus::Ok);
    let mut health = HyphaCycleHealth::default();
    assert_eq!(unsafe { hypha_cycle_health(cycle, &raw mut health) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
    (report.status, health.output_attempts)
}

#[test]
fn commit_without_a_reservation_fails_the_step_abi() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (status, attempts) =
        run_protocol_mistake("commit_alone", ProtocolMistake::CommitWithoutReserve);
    assert_eq!(status, HyphaStatus::InvalidState as i32);
    assert_eq!(attempts, 0);
}

#[test]
fn a_second_reserve_before_commit_fails_the_step_abi() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (status, attempts) =
        run_protocol_mistake("reserve_twice", ProtocolMistake::ReserveWhileOpen);
    assert_eq!(status, HyphaStatus::InvalidState as i32);
    assert_eq!(attempts, 0);
}

#[test]
fn a_reservation_left_open_at_return_fails_the_step_abi() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (status, attempts) = run_protocol_mistake("left_open", ProtocolMistake::ReturnWhileOpen);
    assert_eq!(status, HyphaStatus::InvalidState as i32);
    assert_eq!(attempts, 0);
}

struct CallbackAction {
    action: u32,
    status: i32,
}

extern "C" fn action_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    assert_eq!(unsafe { emit_message(emission, 0, &[7]) }, HyphaStatus::Ok);
    let action = unsafe { &*context.cast::<CallbackAction>() };
    HyphaCallbackResult { action: action.action, status: action.status }
}

fn run_action_cycle(label: &str, action: u32, callback_status: i32) -> (u32, i32, u64) {
    let node = create_node(&unique(label));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, _subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("action_out"))) };
    let mut callback_context = CallbackAction { action, status: callback_status };
    let name = c_text(label);
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                1_000_000,
                Some(action_step),
                (&raw mut callback_context).cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let mut output_index = 0;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                publisher,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut output_index,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, 20_000_000, &raw mut report) }, HyphaStatus::Ok);
    let mut health = HyphaCycleHealth::default();
    assert_eq!(unsafe { hypha_cycle_health(cycle, &raw mut health) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
    (report.reason, report.status, health.output_attempts)
}

#[test]
fn cycle_callback_stop_is_normal_and_discards_outputs() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (reason, status, output_attempts) = run_action_cycle("cycle_stop", HYPHA_ACTION_STOP, 0);
    assert_eq!(reason, HYPHA_RUN_REASON_STOPPED);
    assert_eq!(status, HyphaStatus::Ok as i32);
    assert_eq!(output_attempts, 0);
}

#[test]
fn cycle_callback_continue_status_is_preserved_and_discards_outputs() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let (reason, status, output_attempts) = run_action_cycle(
        "cycle_continue_status",
        HYPHA_ACTION_CONTINUE,
        HyphaStatus::Timeout as i32,
    );
    assert_eq!(reason, HYPHA_RUN_REASON_DEADLINE);
    assert_eq!(status, HyphaStatus::Ok as i32);
    assert_eq!(output_attempts, 0);
}
