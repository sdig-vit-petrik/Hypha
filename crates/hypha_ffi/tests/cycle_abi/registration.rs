//! Cycle C ABI behavior checks.

use super::*;

#[test]
fn cycle_lifecycle_and_validation() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_val"));
    let waitset = unsafe { create_waitset(node) };

    let name = c_text("control_loop");
    let mut cycle = ptr::null_mut();

    assert_eq!(
        unsafe {
            hypha_cycle_create(
                ptr::null_mut(),
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                ptr::null(),
                name.as_bytes().len(),
                10_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                u64::MAX, // beyond i64::MAX; zero makes a triggered cycle
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                None, // callback is None
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                0, // empty name len
                10_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::InvalidArgument
    );

    assert_eq!(
        unsafe {
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
        },
        HyphaStatus::Ok
    );
    assert!(!cycle.is_null());

    // Duplicate cycle name on same waitset
    let mut dup_cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(empty_step),
                ptr::null_mut(),
                None,
                &raw mut dup_cycle,
            )
        },
        HyphaStatus::InvalidArgument
    );

    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    // Double commit returns InvalidState
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidState);

    assert_eq!(unsafe { hypha_cycle_destroy(ptr::null_mut()) }, HyphaStatus::InvalidArgument);

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);

    unsafe {
        hypha_node_destroy(node);
    }
}

#[test]
fn cycle_binding_and_output_ownership() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_bind"));
    let waitset = unsafe { create_waitset(node) };

    let (pub1, sub1) = unsafe { create_pubsub(node, &format!("/{}", unique("b_in"))) };
    let (pub2, _sub2) = unsafe { create_pubsub(node, &format!("/{}", unique("b_out"))) };

    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub1, &raw mut source) },
        HyphaStatus::Ok
    );

    let name = c_text("cycle_bind");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
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
        },
        HyphaStatus::Ok
    );

    let bind_opts = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 1,
    };
    let mut input_idx = 999;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_input(
                ptr::null_mut(),
                source,
                &raw const bind_opts,
                &raw mut input_idx,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe {
            hypha_cycle_bind_input(cycle, ptr::null_mut(), &raw const bind_opts, &raw mut input_idx)
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, ptr::null(), &raw mut input_idx,) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, ptr::null_mut(),) },
        HyphaStatus::InvalidArgument
    );

    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut input_idx,) },
        HyphaStatus::Ok
    );
    assert_eq!(input_idx, 0);

    let invalid_mode = HyphaCycleBindingOptions { mode: u32::MAX, ..bind_opts };
    input_idx = 999;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_input(cycle, source, &raw const invalid_mode, &raw mut input_idx)
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(input_idx, 0);

    let mut output_idx = 999;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                ptr::null_mut(),
                pub2,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut output_idx,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                ptr::null_mut(),
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut output_idx,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                pub2,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                ptr::null_mut(),
            )
        },
        HyphaStatus::InvalidArgument
    );

    // Takes ownership of pub2.
    assert_eq!(
        unsafe { hypha_cycle_bind_output(cycle, pub2, u32::MAX, 1, &raw mut output_idx) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(output_idx, 0);
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                pub2,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut output_idx,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(output_idx, 0);

    // Exclusive publisher ownership: binding pub2 again returns InvalidState
    let mut dup_out_idx = 999;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                pub2,
                HyphaCycleOutputFailure::DropPoolExhausted as u32,
                1,
                &raw mut dup_out_idx,
            )
        },
        HyphaStatus::InvalidState
    );

    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    // Binding after commit returns InvalidState
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_opts, &raw mut input_idx,) },
        HyphaStatus::InvalidState
    );

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub1);
        hypha_node_destroy(node);
    }
}
