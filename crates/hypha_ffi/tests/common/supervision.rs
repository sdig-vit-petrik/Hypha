//! Shared fixtures for input supervision.
#![allow(clippy::expect_used, reason = "test setup must succeed")]

use std::fmt::Write as _;
use std::ptr;

use hypha_ffi::waitset::HYPHA_ACTION_CONTINUE;
use hypha_ffi::*;

use super::{c_text, create_node_with_file, unique};

const DATA_TYPE: &str = "std_msgs/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const ONE_MILLI: i64 = 1_000_000;
const ONE_HOUR: i64 = 3_600_000_000_000;

pub(crate) fn node_with_parameter_text(label: &str, text: &str) -> *mut HyphaNode {
    let dir = std::env::temp_dir().join(unique("supervision_params"));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let path = dir.join("params.toml");
    std::fs::write(&path, text).expect("write parameter fixture");
    create_node_with_file(&unique(label), Some(&path))
}

pub(crate) fn ranges() -> HyphaSupervisionRanges {
    HyphaSupervisionRanges {
        silence_min_nanos: ONE_MILLI,
        silence_max_nanos: ONE_HOUR,
        stagnation_min_nanos: ONE_MILLI,
        stagnation_max_nanos: ONE_HOUR,
    }
}

/// The outputs of one bind call. `detail` is empty when `length` is larger than the buffer.
pub(crate) struct Bound {
    pub(crate) status: HyphaStatus,
    pub(crate) attached: u8,
    pub(crate) ignored: u8,
    pub(crate) limits: HyphaSupervisionLimits,
    pub(crate) length: usize,
    pub(crate) detail: String,
}

/// Bind the supervision of input `binding` as the port `name`, through a detail buffer of
/// `capacity`.
///
/// # Safety
/// `cycle` and `node` must be live handles.
pub(crate) unsafe fn bind_with_capacity(
    cycle: *mut HyphaCycle,
    node: *const HyphaNode,
    binding: u32,
    name: &str,
    checks: (bool, bool, bool),
    selected: &HyphaSupervisionRanges,
    capacity: usize,
) -> Bound {
    let name_c = c_text(name);
    let mut detail = vec![0_u8; capacity];
    let mut length = 0_usize;
    let (silence, stagnation, takes_topic) = checks;
    let request = HyphaInputSupervisionRequest {
        node,
        name: name_c.as_ptr(),
        name_len: name_c.as_bytes().len(),
        // A port that does not require its limit is tagged unsupervised.
        unsupervised: u8::from(!silence),
        check_stagnation: u8::from(stagnation),
        takes_topic: u8::from(takes_topic),
        ranges: *selected,
    };
    let mut result = HyphaInputSupervisionResult::default();
    let status = unsafe {
        hypha_cycle_bind_input_supervision(
            cycle,
            binding,
            &raw const request,
            &raw mut result,
            detail.as_mut_ptr().cast(),
            detail.len(),
            &raw mut length,
        )
    };
    let (attached, ignored, limits) = (result.attached, result.ignored, result.limits);
    let text = detail.get(..length).map(|bytes| String::from_utf8_lossy(bytes).into_owned());
    let text = text.unwrap_or_default();
    Bound { status, attached, ignored, limits, length, detail: text }
}

/// Bind with a detail buffer large enough for any reason the tests use.
///
/// # Safety
/// `cycle` and `node` must be live handles.
pub(crate) unsafe fn bind(
    cycle: *mut HyphaCycle,
    node: *const HyphaNode,
    binding: u32,
    name: &str,
    checks: (bool, bool, bool),
) -> Bound {
    let selected = ranges();
    unsafe { bind_with_capacity(cycle, node, binding, name, checks, &selected, 256) }
}

/// The input port whose table [`node_with_triggers`] gives a silence limit of `silence_ms`.
pub(crate) fn trigger_port(silence_ms: i64) -> String {
    format!("trigger_{silence_ms}ms")
}

/// A node with one `[ports.inputs.trigger_<ms>ms]` table with `silence_ms = <ms>` per limit.
pub(crate) fn node_with_triggers(label: &str, silences_ms: &[i64]) -> *mut HyphaNode {
    let mut text = String::new();
    for ms in silences_ms {
        writeln!(text, "[ports.inputs.{}]\nsilence_ms = {ms}", trigger_port(*ms))
            .expect("write to a String");
    }
    node_with_parameter_text(label, &text)
}

/// Bind the trigger binding `binding` as the port with a silence limit of `silence_ms`.
///
/// # Safety
/// `cycle` and `node` must be live handles; the node came from [`node_with_triggers`].
pub(crate) unsafe fn bind_trigger(
    cycle: *mut HyphaCycle,
    node: *const HyphaNode,
    binding: u32,
    silence_ms: i64,
) -> Bound {
    unsafe { bind(cycle, node, binding, &trigger_port(silence_ms), (true, false, true)) }
}

extern "C" fn no_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut std::ffi::c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A cycle on `node` with one input, bound to a fresh topic. A disabled input reads its
/// keys but attaches nothing.
///
/// # Safety
/// `node` must be a live handle.
pub(crate) unsafe fn cycle_with_input(node: *mut HyphaNode, enabled: bool) -> *mut HyphaCycle {
    let name = unique("cycle");
    unsafe { cycle_with_input_mode(node, enabled, &name, HyphaCycleInputMode::Latest) }
}

/// A cycle called `name` on `node` with one input bound in `mode`.
///
/// # Safety
/// `node` must be a live handle.
pub(crate) unsafe fn cycle_with_input_mode(
    node: *mut HyphaNode,
    enabled: bool,
    name: &str,
    mode: HyphaCycleInputMode,
) -> *mut HyphaCycle {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    let mut source = ptr::null_mut();
    if enabled {
        let topic = c_text(&format!("/{}", unique("supervised_input")));
        let data_type = c_text(DATA_TYPE);
        let md5 = c_text(MD5);
        let qos = HyphaSubscriberOptions {
            depth: 4,
            no_publisher_warning_after_nanos: 0,
            qos: super::test_qos(),
        };
        let mut subscriber = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_subscribe_typed(
                    node,
                    topic.as_ptr(),
                    topic.as_bytes().len(),
                    data_type.as_ptr(),
                    data_type.as_bytes().len(),
                    md5.as_ptr(),
                    md5.as_bytes().len(),
                    std::ptr::null(),
                    0,
                    &raw const qos,
                    &raw mut subscriber,
                )
            },
            HyphaStatus::Ok
        );
        assert_eq!(
            unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
            HyphaStatus::Ok
        );
    } else {
        assert_eq!(
            unsafe { hypha_input_source_create(waitset, ptr::null_mut(), &raw mut source) },
            HyphaStatus::Ok
        );
    }

    let name = c_text(name);
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(no_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let bind_options =
        HyphaCycleBindingOptions { mode: mode as u32, max_samples_per_iteration: 1, optional: 0 };
    let mut index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind_options, &raw mut index) },
        HyphaStatus::Ok
    );
    cycle
}
