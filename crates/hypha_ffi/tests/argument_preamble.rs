//! The argument preamble of every exported function, in one table.
//! A function the header declares and the table does not name fails
//! `every_header_function_has_a_row`.

mod common;
use crate::common::replay::{create_pubsub, create_waitset};
use crate::common::{c_text, create_node, empty_config_value, unique};

use std::ptr::{null, null_mut};

use hypha_ffi::*;

/// Live handles that a row with a valid handle and one invalid argument uses.
struct Live {
    node: *mut HyphaNode,
    config: *const HyphaConfig,
    waitset: *mut HyphaWaitSet,
    publisher: *mut HyphaPublisher,
    subscriber: *mut HyphaSubscriber,
}

impl Live {
    fn new() -> Self {
        let node = create_node(&unique("preamble"));
        let config = unsafe { hypha_node_config(node) };
        let waitset = unsafe { create_waitset(node) };
        let (publisher, subscriber) =
            unsafe { create_pubsub(node, &format!("/{}", unique("preamble")), 1) };
        Self { node, config, waitset, publisher, subscriber }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        unsafe {
            hypha_subscriber_destroy(self.subscriber);
            hypha_publisher_destroy(self.publisher);
            hypha_node_destroy(self.node);
        }
    }
}

const INVALID: HyphaStatus = HyphaStatus::InvalidArgument;

/// The functions that take no pointer and no code to refuse. Each names its reason.
const NO_PREAMBLE: &[(&str, &str)] = &[
    ("hypha_abi_revision", "no argument"),
    ("hypha_clock_now_nanos", "no argument"),
    ("hypha_clock_steady_now_nanos", "no argument"),
    ("hypha_clock_is_sim_time", "no argument"),
    ("hypha_clock_is_valid", "no argument"),
    ("hypha_clock_disable_sim_time", "no argument"),
    ("hypha_clock_enable_sim_time", "a negative start is refused; abi_boundary_values"),
    ("hypha_clock_set_sim_time", "a negative time is refused; abi_boundary_values"),
    ("hypha_clock_advance_sim_time", "a negative span is refused; abi_boundary_values"),
    ("hypha_log_enabled", "an unknown module or level reads as disabled; log_boundary"),
    ("hypha_log_write", "a null text with a length is counted as rejected; log_boundary"),
];

/// Step-test services with no configuration and no port, for the rows that need a live one.
fn step_services() -> *mut HyphaStepServices {
    let mut out = null_mut();
    // SAFETY: a null configuration is allowed; the name and `out` are valid.
    let status =
        unsafe { hypha_step_services_create(null(), "c".as_ptr().cast(), 1, &raw mut out) };
    assert_eq!(status, HyphaStatus::Ok);
    out
}

extern "C" fn step_ok(_emission: *mut HyphaCycleEmission, _context: *mut std::ffi::c_void) -> u8 {
    1
}

extern "C" fn request_state(_state: *const HyphaStepRequestState, _context: *mut std::ffi::c_void) {
}

extern "C" fn sent_call(
    _request: *const u8,
    _len: usize,
    _sent: i64,
    _context: *mut std::ffi::c_void,
) {
}

/// The releases `count_release` saw, over every row.
static RELEASED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn released() -> usize {
    RELEASED.load(std::sync::atomic::Ordering::SeqCst)
}

extern "C" fn count_release(_context: *mut std::os::raw::c_void) {
    RELEASED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

extern "C" fn serve(
    _view: *const HyphaRequestView,
    _context: *mut std::os::raw::c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult::continue_run()
}

extern "C" fn done(
    _result: *const HyphaCallResult,
    _context: *mut std::os::raw::c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult::continue_run()
}

struct Row {
    name: &'static str,
    check: fn(&Live),
}

macro_rules! rows {
    ($($name:ident => |$live:ident| $body:block)*) => {
        &[$(Row { name: stringify!($name), check: |$live: &Live| $body }),*]
    };
}

/// Every function with a preamble, in header order.
#[allow(clippy::too_many_lines, reason = "one row per exported function")]
static ROWS: &[Row] = rows! {
    hypha_config_create => |_live| {
        assert_eq!(unsafe { hypha_config_create(null(), 0, null_mut()) }, INVALID);
    }
    hypha_config_destroy => |_live| { unsafe { hypha_config_destroy(null_mut()) } }
    hypha_node_config => |_live| { assert_eq!(unsafe { hypha_node_config(null()) }, null()) }
    hypha_config_unread_keys => |live| {
        assert_eq!(
            unsafe { hypha_config_unread_keys(null(), null(), 0, null_mut(), 0, &mut 0) },
            INVALID
        );
        assert_eq!(
            unsafe { hypha_config_unread_keys(live.config, null(), 0, null_mut(), 0, null_mut()) },
            INVALID
        );
    }
    hypha_config_table_names => |live| {
        assert_eq!(
            unsafe { hypha_config_table_names(null(), null(), 0, null_mut(), 0, &mut 0) },
            INVALID
        );
        assert_eq!(
            unsafe { hypha_config_table_names(live.config, null(), 0, null_mut(), 0, null_mut()) },
            INVALID
        );
        assert_eq!(
            unsafe { hypha_config_table_names(live.config, null(), 1, null_mut(), 0, &mut 0) },
            INVALID
        );
    }
    hypha_config_ignore => |live| {
        let key = "a".as_ptr().cast();
        let reason = "b".as_ptr().cast();
        // SAFETY: a null configuration is refused before either text is read.
        let null_config = unsafe { hypha_config_ignore(null(), key, 1, reason, 1) };
        assert_eq!(null_config, INVALID);
        // SAFETY: a null key with a length is refused before it is read.
        let null_key = unsafe { hypha_config_ignore(live.config, null(), 1, reason, 1) };
        assert_eq!(null_key, INVALID);
        // SAFETY: a null reason with a length is refused before it is read.
        let null_reason = unsafe { hypha_config_ignore(live.config, key, 1, null(), 1) };
        assert_eq!(null_reason, INVALID);
    }
    hypha_config_resolve_key => |live| {
        let mut length = 0_usize;
        let name = "map";
        assert_eq!(
            unsafe {
                hypha_config_resolve_key(
                    null(), name.as_ptr().cast(), name.len(), null_mut(), 0, &raw mut length,
                )
            },
            INVALID
        );
        assert_eq!(
            unsafe {
                hypha_config_resolve_key(live.config, null(), 0, null_mut(), 0, &raw mut length)
            },
            INVALID,
            "an empty key"
        );
        assert_eq!(
            unsafe {
                hypha_config_resolve_key(
                    live.config, name.as_ptr().cast(), name.len(), null_mut(), 0, null_mut(),
                )
            },
            INVALID,
            "a null length"
        );
    }
    hypha_cycle_set_clock_driver => |_live| {
        assert_eq!(unsafe { hypha_cycle_set_clock_driver(null_mut(), null()) }, INVALID);
    }
    hypha_cycle_expect_participant => |_live| {
        assert_eq!(unsafe { hypha_cycle_expect_participant(null_mut(), null(), 0) }, INVALID);
    }
    hypha_cycle_image_sample => |_live| {
        assert_eq!(unsafe { hypha_cycle_image_sample(null(), 0, 0, null_mut()) }, INVALID);
    }
    hypha_cycle_emission_reserve => |_live| {
        assert_eq!(unsafe { hypha_cycle_emission_reserve(null_mut(), 0, 0, null_mut()) }, INVALID);
    }
    hypha_cycle_emission_commit => |_live| {
        assert_eq!(unsafe { hypha_cycle_emission_commit(null_mut()) }, INVALID);
    }
    hypha_cycle_image_binding => |_live| {
        assert_eq!(unsafe { hypha_cycle_image_binding(null(), 0, null_mut()) }, INVALID);
    }
    hypha_cycle_image_held => |_live| {
        assert_eq!(unsafe { hypha_cycle_image_held(null(), 0, null_mut()) }, INVALID);
    }
    hypha_cycle_image_reject_sample => |_live| {
        assert_eq!(unsafe { hypha_cycle_image_reject_sample(null(), 0, 0, null_mut()) }, INVALID);
    }
    hypha_cycle_image_alive => |_live| {
        assert_eq!(unsafe { hypha_cycle_image_alive(null(), 0, 0) }, INVALID);
    }
    hypha_cycle_image_clear => |_live| {
        assert_eq!(unsafe { hypha_cycle_image_clear(null(), HYPHA_CYCLE_CLEAR_INPUT, 0) }, INVALID);
    }
    hypha_cycle_image_set_ignored => |_live| {
        assert_eq!(unsafe { hypha_cycle_image_set_ignored(null(), 0, 0) }, INVALID);
    }
    hypha_cycle_bind_input_supervision => |_live| {
        assert_eq!(
            unsafe {
                hypha_cycle_bind_input_supervision(
                    null_mut(), 0, null(), null_mut(), null_mut(), 0, &mut 0,
                )
            },
            INVALID
        );
    }
    hypha_cycle_health => |_live| {
        assert_eq!(unsafe { hypha_cycle_health(null(), null_mut()) }, INVALID);
    }
    hypha_cycle_binding => |_live| {
        assert_eq!(unsafe { hypha_cycle_binding(null(), 0, null_mut()) }, INVALID);
    }
    hypha_cycle_create => |_live| {
        assert_eq!(
            unsafe {
                hypha_cycle_create(null_mut(), null(), 0, 0, None, null_mut(), None, &mut null_mut())
            },
            INVALID
        );
    }
    hypha_cycle_bind_input => |_live| {
        assert_eq!(
            unsafe { hypha_cycle_bind_input(null_mut(), null_mut(), null(), &mut 0) },
            INVALID
        );
    }
    hypha_cycle_bind_output => |_live| {
        assert_eq!(
            unsafe { hypha_cycle_bind_output(null_mut(), null_mut(), 0, 1, &mut 0) },
            INVALID
        );
    }
    hypha_cycle_set_trigger => |_live| {
        assert_eq!(unsafe { hypha_cycle_set_trigger(null_mut(), 0) }, INVALID);
    }
    hypha_cycle_set_host_clock => |_live| {
        assert_eq!(unsafe { hypha_cycle_set_host_clock(null_mut(), 0) }, INVALID);
    }
    hypha_cycle_set_class_based => |_live| {
        assert_eq!(unsafe { hypha_cycle_set_class_based(null_mut(), 1) }, INVALID);
    }
    hypha_cycle_commit => |_live| { assert_eq!(unsafe { hypha_cycle_commit(null_mut()) }, INVALID) }
    hypha_cycle_destroy => |_live| { assert_eq!(unsafe { hypha_cycle_destroy(null_mut()) }, INVALID) }
    hypha_input_source_create => |_live| {
        assert_eq!(
            unsafe { hypha_input_source_create(null_mut(), null_mut(), &mut null_mut()) },
            INVALID
        );
    }
    hypha_input_source_destroy => |_live| {
        assert_eq!(unsafe { hypha_input_source_destroy(null_mut()) }, INVALID);
    }
    hypha_cycle_bind_transforms => |_live| {
        assert_eq!(
            unsafe { hypha_cycle_bind_transforms(null_mut(), null_mut(), null_mut(), 1, &mut 0) },
            INVALID
        );
    }
    hypha_cycle_image_lookup_transform => |_live| {
        assert_eq!(
            unsafe {
                hypha_cycle_image_lookup_transform(
                    null(), 0, null(), 0, null(), 0, 0, 0, null_mut(), null_mut(),
                )
            },
            INVALID
        );
    }
    hypha_node_record_config => |live| {
        let key = c_text("period");
        let value = c_text("50");
        assert_eq!(
            unsafe { hypha_node_record_config(null_mut(), null(), null(), null(), 0) },
            INVALID
        );
        assert_eq!(
            unsafe {
                hypha_node_record_config(
                    live.node, null(), value.as_ptr(), null(), HYPHA_CONFIG_SOURCE_APPLICATION,
                )
            },
            INVALID,
            "a null key"
        );
        assert_eq!(
            unsafe { hypha_node_record_config(live.node, key.as_ptr(), value.as_ptr(), null(), 99) },
            INVALID,
            "an unknown source"
        );
    }
    hypha_config_record_config => |_live| {
        assert_eq!(
            unsafe { hypha_config_record_config(null(), null(), null(), null(), 0) },
            INVALID
        );
    }
    hypha_node_introspect => |live| {
        let mut length = 0_usize;
        assert_eq!(
            unsafe { hypha_node_introspect(null(), null_mut(), 0, &raw mut length) },
            INVALID
        );
        assert_eq!(
            unsafe { hypha_node_introspect(live.node, null_mut(), 0, null_mut()) },
            INVALID,
            "a null length"
        );
        assert_eq!(
            unsafe { hypha_node_introspect(live.node, null_mut(), 8, &raw mut length) },
            INVALID,
            "a null buffer with a capacity"
        );
    }
    hypha_node_state => |live| {
        let mut length = 0_usize;
        assert_eq!(unsafe { hypha_node_state(null(), null_mut(), 0, &raw mut length) }, INVALID);
        assert_eq!(
            unsafe { hypha_node_state(live.node, null_mut(), 0, null_mut()) },
            INVALID,
            "a null length"
        );
        assert_eq!(
            unsafe { hypha_node_state(live.node, null_mut(), 8, &raw mut length) },
            INVALID,
            "a null buffer with a capacity"
        );
        assert_eq!(
            unsafe { hypha_node_state(live.node, null_mut(), 0, &raw mut length) },
            HyphaStatus::TooLarge,
            "the size query"
        );
        assert!(length > 0);
    }
    hypha_last_error => |_live| {
        assert_eq!(unsafe { hypha_last_error(null_mut(), 0, null_mut()) }, INVALID);
    }
    hypha_publisher_loan_frame => |_live| {
        assert_eq!(
            unsafe { hypha_publisher_loan_frame(null_mut(), 0, &mut null_mut(), &mut null_mut()) },
            INVALID
        );
    }
    hypha_publisher_reserve_frame => |_live| {
        assert_eq!(
            unsafe { hypha_publisher_reserve_frame(null_mut(), 0, &mut null_mut(), &mut null_mut()) },
            INVALID
        );
    }
    hypha_loan_send => |_live| { assert_eq!(unsafe { hypha_loan_send(null_mut(), -1) }, INVALID) }
    hypha_loan_drop => |_live| { unsafe { hypha_loan_drop(null_mut()) } }
    hypha_loan_shrink => |live| {
        assert_eq!(unsafe { hypha_loan_shrink(null_mut(), 0) }, INVALID);
        let mut loan = null_mut();
        let mut payload = null_mut();
        assert_eq!(
            unsafe { hypha_publisher_loan_frame(live.publisher, 8, &raw mut loan, &raw mut payload) },
            HyphaStatus::Ok
        );
        assert_eq!(unsafe { hypha_loan_shrink(loan, 9) }, INVALID);
        assert_eq!(unsafe { hypha_loan_shrink(loan, 3) }, HyphaStatus::Ok);
        unsafe { hypha_loan_drop(loan) };
    }
    hypha_log_module_register => |_live| {
        let mut id: HyphaLogModule = 0;
        assert_eq!(unsafe { hypha_log_module_register(null(), 3, &raw mut id) }, INVALID);
        assert_eq!(
            unsafe { hypha_log_module_register(c"abc".as_ptr(), 3, null_mut()) },
            INVALID
        );
    }
    hypha_node_metrics => |live| {
        assert_eq!(unsafe { hypha_node_metrics(null(), None, null_mut()) }, INVALID);
        assert_eq!(
            unsafe { hypha_node_metrics(live.node, None, null_mut()) },
            INVALID,
            "a missing callback"
        );
    }
    hypha_node_name => |live| {
        assert_eq!(unsafe { hypha_node_name(null(), null_mut(), 0, &mut 0) }, INVALID);
        assert_eq!(unsafe { hypha_node_name(live.node, null_mut(), 0, null_mut()) }, INVALID);
    }
    hypha_node_create => |_live| {
        assert_eq!(
            unsafe { hypha_node_create(null(), 0, null(), 0, null(), null_mut()) },
            INVALID
        );
        let invalid_name = [0xff_u8];
        let mut node = null_mut();
        assert_eq!(
            unsafe {
                hypha_node_create(
                    invalid_name.as_ptr().cast(), 1, null(), 0, null(), &raw mut node,
                )
            },
            INVALID,
            "a name that is not UTF-8"
        );
        assert!(node.is_null());
    }
    hypha_node_destroy => |_live| { unsafe { hypha_node_destroy(null_mut()) } }
    hypha_node_ok => |_live| { assert_eq!(unsafe { hypha_node_ok(null()) }, 0) }
    hypha_node_shutdown => |_live| { unsafe { hypha_node_shutdown(null_mut()) } }
    hypha_list_nodes => |_live| {
        assert_eq!(unsafe { hypha_list_nodes(None, null_mut()) }, INVALID);
    }
    hypha_topic_validate_public_name => |_live| {
        assert_eq!(unsafe { hypha_topic_validate_public_name(null(), 3) }, INVALID);
    }
    hypha_list_service_ports => |live| {
        assert_eq!(unsafe { hypha_list_service_ports(null(), None, null_mut()) }, INVALID);
        assert_eq!(
            unsafe { hypha_list_service_ports(live.node, None, null_mut()) },
            INVALID,
            "a missing callback"
        );
    }
    hypha_param_topic => |_live| {
        assert_eq!(
            unsafe {
                hypha_param_topic(
                    null(), HYPHA_PORT_INPUT, null(), 0, HYPHA_PORT_TOPIC_REQUIRED,
                    null_mut(), 0, &mut 0,
                )
            },
            INVALID
        );
    }
    hypha_param_validate_number => |_live| {
        assert_eq!(hypha_param_validate_number(1.0, 99, 0.0, 0.0), INVALID);
    }
    hypha_param_validate_nonempty => |_live| {
        assert_eq!(unsafe { hypha_param_validate_nonempty(null(), 3) }, INVALID);
    }
    hypha_config_value => |live| {
        let name = b"value";
        let mut out = empty_config_value();
        let result =
            unsafe { hypha_config_value(null(), name.as_ptr().cast(), name.len(), &raw mut out) };
        assert_eq!((result, out.kind), (HyphaParamResult::InvalidArgument, u32::MAX));
        assert_eq!(
            unsafe { hypha_config_value(live.config, null(), 0, &raw mut out) },
            HyphaParamResult::InvalidArgument,
            "a null name"
        );
        assert_eq!(
            unsafe { hypha_config_value(live.config, name.as_ptr().cast(), name.len(), null_mut()) },
            HyphaParamResult::InvalidArgument,
            "a null output"
        );
    }
    hypha_config_list_len => |live| {
        let list = b"values";
        let mut out_len = 77_usize;
        let result = unsafe {
            hypha_config_list_len(null(), list.as_ptr().cast(), list.len(), &raw mut out_len)
        };
        assert_eq!((result, out_len), (HyphaParamResult::InvalidArgument, 77));
        assert_eq!(
            unsafe { hypha_config_list_len(live.config, null(), 0, &raw mut out_len) },
            HyphaParamResult::InvalidArgument,
            "a null name"
        );
        assert_eq!(
            unsafe { hypha_config_list_len(live.config, list.as_ptr().cast(), list.len(), null_mut()) },
            HyphaParamResult::InvalidArgument,
            "a null output"
        );
    }
    hypha_config_list_value => |live| {
        let list = b"values";
        let field = b"";
        let mut out = empty_config_value();
        let result = unsafe {
            hypha_config_list_value(
                null(), list.as_ptr().cast(), list.len(), 0, field.as_ptr().cast(), 0, &raw mut out,
            )
        };
        assert_eq!((result, out.kind), (HyphaParamResult::InvalidArgument, u32::MAX));
        assert_eq!(
            unsafe {
                hypha_config_list_value(
                    live.config, list.as_ptr().cast(), list.len(), 0, null(), 0, &raw mut out,
                )
            },
            HyphaParamResult::InvalidArgument,
            "a null field is refused; an empty field is the element itself"
        );
        assert_eq!(
            unsafe {
                hypha_config_list_value(
                    live.config, list.as_ptr().cast(), list.len(), 0, field.as_ptr().cast(), 0,
                    null_mut(),
                )
            },
            HyphaParamResult::InvalidArgument,
            "a null output"
        );
    }
    hypha_publish_with_qos => |_live| {
        assert_eq!(
            unsafe {
                hypha_publish_with_qos(
                    null_mut(), null(), 0, null(), 0, null(), 0, null(), 0, null(), &mut null_mut(),
                )
            },
            INVALID
        );
    }
    hypha_publisher_destroy => |_live| { unsafe { hypha_publisher_destroy(null_mut()) } }
    hypha_publisher_subscriber_count => |_live| {
        let mut count = 77_usize;
        assert_eq!(unsafe { hypha_publisher_subscriber_count(null(), &raw mut count) }, 0);
        assert_eq!(count, 77, "an unanswered count writes nothing");
    }
    hypha_publisher_min_subscriber_buffer => |live| {
        assert_eq!(unsafe { hypha_publisher_min_subscriber_buffer(null(), &mut 0) }, INVALID);
        assert_eq!(
            unsafe { hypha_publisher_min_subscriber_buffer(live.publisher, null_mut()) },
            INVALID
        );
    }
    hypha_schema_lookup => |_live| {
        assert_eq!(
            unsafe {
                hypha_schema_lookup(null(), null(), 0, null(), 0, null(), 0, null_mut(), 0, &mut 0)
            },
            INVALID
        );
    }
    hypha_subscribe_typed => |_live| {
        assert_eq!(
            unsafe {
                hypha_subscribe_typed(
                    null_mut(), null(), 0, null(), 0, null(), 0, std::ptr::null(), 0, null(), &mut null_mut(),
                )
            },
            INVALID
        );
    }
    hypha_subscriber_publisher_count => |live| {
        assert_eq!(unsafe { hypha_subscriber_publisher_count(null(), null_mut()) }, INVALID);
        assert_eq!(
            unsafe { hypha_subscriber_publisher_count(live.subscriber, null_mut()) },
            INVALID
        );
    }
    hypha_subscriber_counters => |live| {
        let mut out = HyphaSubscriberCounters { loan_budget: 7, ..HyphaSubscriberCounters::default() };
        assert_eq!(unsafe { hypha_subscriber_counters(null(), &raw mut out) }, INVALID);
        assert_eq!(out.loan_budget, 7, "a refused call writes nothing");
        assert_eq!(unsafe { hypha_subscriber_counters(live.subscriber, null_mut()) }, INVALID);
    }
    hypha_subscriber_missing_sequences => |live| {
        let mut count = u64::MAX;
        assert_eq!(unsafe { hypha_subscriber_missing_sequences(null(), &raw mut count) }, INVALID);
        assert_eq!(count, u64::MAX, "a refused call writes nothing");
        assert_eq!(
            unsafe { hypha_subscriber_missing_sequences(live.subscriber, null_mut()) },
            INVALID
        );
    }
    hypha_subscriber_topic => |live| {
        assert_eq!(unsafe { hypha_subscriber_topic(null(), null_mut(), 0, &mut 0) }, INVALID);
        assert_eq!(
            unsafe { hypha_subscriber_topic(live.subscriber, null_mut(), 0, null_mut()) },
            INVALID
        );
    }
    hypha_subscriber_destroy => |_live| { unsafe { hypha_subscriber_destroy(null_mut()) } }
    hypha_transform_buffer_create => |live| {
        let topic = "/preamble_tf";
        let other = "/preamble_tf_static";
        let qos = crate::common::test_qos();
        let create = |node, qos, out| unsafe {
            hypha_transform_buffer_create(
                node,
                topic.as_ptr().cast(),
                topic.len(),
                other.as_ptr().cast(),
                other.len(),
                qos,
                1,
                out,
            )
        };
        let mut out = null_mut();
        assert_eq!(create(null_mut(), &raw const qos, &raw mut out), INVALID);
        assert!(out.is_null(), "a refused create writes null");
        assert_eq!(create(live.node, null(), &raw mut out), INVALID);
        assert_eq!(create(live.node, &raw const qos, null_mut()), INVALID);
    }
    hypha_transform_buffer_create_fed => |_live| {
        let mut out = null_mut();
        assert_eq!(unsafe { hypha_transform_buffer_create_fed(0, &raw mut out) }, INVALID);
        assert!(out.is_null(), "a refused create writes null");
        assert_eq!(unsafe { hypha_transform_buffer_create_fed(1, null_mut()) }, INVALID);
    }
    hypha_transform_buffer_feed => |_live| {
        assert_eq!(unsafe { hypha_transform_buffer_feed(null(), [0_u8].as_ptr(), 1, 0) }, INVALID);
    }
    hypha_transform_buffer_lookup => |_live| {
        let mut result = 9_u32;
        let mut out = HyphaTransform::default();
        assert_eq!(
            unsafe {
                hypha_transform_buffer_lookup(
                    null(), "a".as_ptr().cast(), 1, "a".as_ptr().cast(), 1, 0, 1,
                    &raw mut result, &raw mut out,
                )
            },
            INVALID
        );
        assert_eq!(result, 9, "a refused lookup writes nothing");
    }
    hypha_transform_buffer_destroy => |_live| {
        assert_eq!(unsafe { hypha_transform_buffer_destroy(null_mut()) }, HyphaStatus::Ok);
    }
    hypha_transform_check => |_live| {
        let name = "a".as_ptr().cast();
        let sent = HyphaSentTransform { rotation_xyzw: [0.0, 0.0, 0.0, 1.0], ..HyphaSentTransform::default() };
        // SAFETY: a null parent with a length is refused before anything is read.
        let no_parent = unsafe { hypha_transform_check(null(), 1, name, 1, &raw const sent, 0) };
        assert_eq!(no_parent, INVALID);
        // SAFETY: a null child with a length is refused before anything is read.
        let no_child = unsafe { hypha_transform_check(name, 1, null(), 1, &raw const sent, 0) };
        assert_eq!(no_child, INVALID);
        // SAFETY: a null transform is refused before it is read.
        let no_transform = unsafe { hypha_transform_check(name, 1, "b".as_ptr().cast(), 1, null(), 0) };
        assert_eq!(no_transform, INVALID);
    }
    hypha_service_server_create => |live| {
        // SAFETY: every field of a description accepts all-zero bits.
        let desc: HyphaServiceDesc = unsafe { std::mem::zeroed() };
        let mut out = null_mut();
        let before = released();
        // SAFETY: a null node is refused before the description is read.
        let no_node = unsafe {
            hypha_service_server_create(null_mut(), &raw const desc, Some(serve), null_mut(), Some(count_release), &raw mut out)
        };
        assert_eq!(no_node, INVALID);
        // SAFETY: a missing handler is refused before the description is read.
        let no_handler = unsafe {
            hypha_service_server_create(live.node, &raw const desc, None, null_mut(), Some(count_release), &raw mut out)
        };
        assert_eq!(no_handler, INVALID);
        assert_eq!(released().wrapping_sub(before), 2, "a refused create releases its context, once each");
        assert!(out.is_null());
    }
    hypha_service_server_destroy => |_live| { unsafe { hypha_service_server_destroy(null_mut()) } }
    hypha_request_answer => |_live| {
        assert_eq!(unsafe { hypha_request_answer(null_mut(), HYPHA_ANSWER_REPLY, null(), 0) }, INVALID);
    }
    hypha_request_defer => |_live| {
        assert_eq!(unsafe { hypha_request_defer(null_mut(), &mut null_mut()) }, INVALID);
    }
    hypha_request_destroy => |_live| { unsafe { hypha_request_destroy(null_mut()) } }
    hypha_service_client_create => |live| {
        // SAFETY: every field of a description accepts all-zero bits.
        let mut desc: HyphaServiceDesc = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { hypha_service_client_create(null_mut(), &raw const desc, &mut null_mut()) }, INVALID);
        assert_eq!(unsafe { hypha_service_client_create(live.node, null(), &mut null_mut()) }, INVALID);
        desc.blocking = 2;
        assert_eq!(unsafe { hypha_service_client_create(live.node, &raw const desc, &mut null_mut()) }, INVALID);
    }
    hypha_service_client_destroy => |_live| { unsafe { hypha_service_client_destroy(null_mut()) } }
    hypha_client_call => |_live| {
        let before = released();
        let mut status = 0_u32;
        // SAFETY: a null client is refused before the request is read.
        let refused = unsafe {
            hypha_client_call(null(), null(), 0, Some(done), null_mut(), Some(count_release), &raw mut status)
        };
        assert_eq!(refused, INVALID);
        assert_eq!(released().wrapping_sub(before), 1, "a refused call releases its context once");
    }
    hypha_client_status => |_live| {
        assert_eq!(unsafe { hypha_client_status(null(), null_mut()) }, INVALID);
    }
    hypha_client_wait_for_server => |_live| {
        assert_eq!(unsafe { hypha_client_wait_for_server(null(), 0, null_mut()) }, INVALID);
    }
    hypha_cycle_bind_server => |_live| {
        // SAFETY: every field of a description accepts all-zero bits.
        let desc: HyphaServiceDesc = unsafe { std::mem::zeroed() };
        let mut index = 7_u32;
        // SAFETY: a null cycle is refused before the description is read.
        assert_eq!(unsafe { hypha_cycle_bind_server(null_mut(), &raw const desc, &raw mut index) }, INVALID);
        assert_eq!(index, 0, "a refused bind writes the index first");
        assert_eq!(unsafe { hypha_cycle_bind_server(null_mut(), &raw const desc, null_mut()) }, INVALID);
    }
    hypha_cycle_bind_client => |_live| {
        // SAFETY: every field of a description accepts all-zero bits.
        let desc: HyphaServiceDesc = unsafe { std::mem::zeroed() };
        // SAFETY: a null cycle is refused before the description is read.
        assert_eq!(unsafe { hypha_cycle_bind_client(null_mut(), &raw const desc, &mut 0) }, INVALID);
        assert_eq!(unsafe { hypha_cycle_bind_client(null_mut(), &raw const desc, null_mut()) }, INVALID);
    }
    hypha_cycle_requests => |_live| {
        // SAFETY: a null emission is refused before anything is read.
        assert_eq!(unsafe { hypha_cycle_requests(null_mut(), 0, Some(serve), null_mut()) }, INVALID);
        assert_eq!(unsafe { hypha_cycle_requests(null_mut(), 0, None, null_mut()) }, INVALID);
    }
    hypha_cycle_next_result => |_live| {
        let mut found = 1_u8;
        // SAFETY: a null emission is refused before anything is read.
        let refused = unsafe { hypha_cycle_next_result(null_mut(), 0, Some(done), null_mut(), &raw mut found) };
        assert_eq!(refused, INVALID);
        assert_eq!(found, 0, "a refused read writes found first");
        assert_eq!(unsafe { hypha_cycle_next_result(null_mut(), 0, Some(done), null_mut(), null_mut()) }, INVALID);
    }
    hypha_cycle_unread_results => |_live| {
        let mut unread = 9_u64;
        // SAFETY: a null emission is refused before anything is read.
        assert_eq!(unsafe { hypha_cycle_unread_results(null_mut(), 0, &raw mut unread) }, INVALID);
        assert_eq!(unread, 0, "a refused read writes 0 first");
        assert_eq!(unsafe { hypha_cycle_unread_results(null_mut(), 0, null_mut()) }, INVALID);
    }
    hypha_cycle_call => |_live| {
        let mut status = 0_u32;
        // SAFETY: a null emission is refused before the request is read.
        assert_eq!(unsafe { hypha_cycle_call(null_mut(), 0, null(), 0, &raw mut status) }, INVALID);
        assert_eq!(status, HYPHA_CALL_BUSY, "a refused call writes Busy first");
        assert_eq!(unsafe { hypha_cycle_call(null_mut(), 0, null(), 0, null_mut()) }, INVALID);
    }
    hypha_step_services_create => |live| {
        let mut out = null_mut();
        // SAFETY: a null output is refused before anything is read.
        assert_eq!(unsafe { hypha_step_services_create(live.config, "c".as_ptr().cast(), 1, null_mut()) }, INVALID);
        // SAFETY: a null name with a length is refused, and `out` is written null first.
        assert_eq!(unsafe { hypha_step_services_create(null(), null(), 1, &raw mut out) }, INVALID);
        assert!(out.is_null(), "a refused create writes null");
    }
    hypha_step_services_destroy => |_live| {
        assert_eq!(unsafe { hypha_step_services_destroy(null_mut()) }, HyphaStatus::Ok);
    }
    hypha_step_services_bind => |_live| {
        // SAFETY: every field of a description accepts all-zero bits.
        let desc: HyphaServiceDesc = unsafe { std::mem::zeroed() };
        let mut index = 7_u32;
        // SAFETY: null services are refused before the description is read.
        assert_eq!(unsafe { hypha_step_services_bind(null_mut(), &raw const desc, 0, &raw mut index) }, INVALID);
        assert_eq!(index, 0, "a refused bind writes the index first");
        let services = step_services();
        // SAFETY: a null description is refused before anything is read.
        assert_eq!(unsafe { hypha_step_services_bind(services, null(), 1, &raw mut index) }, INVALID);
        // SAFETY: `services` is live and not used after.
        unsafe { hypha_step_services_destroy(services) };
    }
    hypha_step_services_deliver => |_live| {
        let mut id = 0_u64;
        // SAFETY: null services are refused before the request is read.
        let refused = unsafe { hypha_step_services_deliver(null_mut(), 0, null(), 0, null(), 0, 0, 0, &raw mut id) };
        assert_eq!(refused, INVALID);
        assert_eq!(id, u64::MAX, "a refused delivery writes no id");
        let services = step_services();
        // SAFETY: an unknown server is refused.
        let unknown = unsafe { hypha_step_services_deliver(services, 0, null(), 0, "t".as_ptr().cast(), 1, 0, 0, &raw mut id) };
        assert_eq!(unknown, INVALID);
        // SAFETY: `services` is live and not used after.
        unsafe { hypha_step_services_destroy(services) };
    }
    hypha_step_services_feed => |_live| {
        // SAFETY: null services are refused before the bytes are read.
        assert_eq!(unsafe { hypha_step_services_feed(null_mut(), 0, HYPHA_CALL_ANSWERED, null(), 0, 0, 0) }, INVALID);
        let services = step_services();
        // SAFETY: an unknown outcome is refused.
        assert_eq!(unsafe { hypha_step_services_feed(services, 0, 99, null(), 0, 0, 0) }, INVALID);
        // SAFETY: `services` is live and not used after.
        unsafe { hypha_step_services_destroy(services) };
    }
    hypha_step_services_step => |_live| {
        // SAFETY: null services are refused before the step runs.
        assert_eq!(unsafe { hypha_step_services_step(null_mut(), 0, Some(step_ok), null_mut()) }, INVALID);
        let services = step_services();
        // SAFETY: a missing step is refused.
        assert_eq!(unsafe { hypha_step_services_step(services, 0, None, null_mut()) }, INVALID);
        // SAFETY: `services` is live and not used after.
        unsafe { hypha_step_services_destroy(services) };
    }
    hypha_step_services_request => |_live| {
        // SAFETY: null services are refused before the callback runs.
        assert_eq!(unsafe { hypha_step_services_request(null(), 0, 0, Some(request_state), null_mut()) }, INVALID);
        let services = step_services();
        // SAFETY: a missing callback and an unknown server are refused.
        assert_eq!(unsafe { hypha_step_services_request(services, 0, 0, None, null_mut()) }, INVALID);
        assert_eq!(unsafe { hypha_step_services_request(services, 0, 0, Some(request_state), null_mut()) }, INVALID);
        // SAFETY: `services` is live and not used after.
        unsafe { hypha_step_services_destroy(services) };
    }
    hypha_step_services_calls => |_live| {
        // SAFETY: null services are refused before the callback runs.
        assert_eq!(unsafe { hypha_step_services_calls(null(), 0, Some(sent_call), null_mut()) }, INVALID);
        let services = step_services();
        // SAFETY: an unknown client is refused.
        assert_eq!(unsafe { hypha_step_services_calls(services, 0, Some(sent_call), null_mut()) }, INVALID);
        // SAFETY: `services` is live and not used after.
        unsafe { hypha_step_services_destroy(services) };
    }
    hypha_node_default_waitset => |live| {
        assert_eq!(unsafe { hypha_node_default_waitset(null_mut(), &mut null_mut()) }, INVALID);
        assert_eq!(unsafe { hypha_node_default_waitset(live.node, null_mut()) }, INVALID);
    }
    hypha_waitset_attach => |live| {
        // SAFETY: every field of an attachment accepts all-zero bits.
        let record: HyphaAttachment = unsafe { std::mem::zeroed() };
        let mut out: HyphaAttachmentId = 0;
        assert_eq!(unsafe { hypha_waitset_attach(null_mut(), &raw const record, &raw mut out) }, INVALID);
        assert_eq!(unsafe { hypha_waitset_attach(live.waitset, null(), &raw mut out) }, INVALID);
        assert_eq!(
            unsafe { hypha_waitset_attach(live.waitset, &raw const record, null_mut()) },
            INVALID
        );
    }
    hypha_waitset_detach => |_live| { assert_eq!(unsafe { hypha_waitset_detach(null_mut(), 0) }, INVALID) }
    hypha_waitset_run => |live| {
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        assert_eq!(unsafe { hypha_waitset_run(null_mut(), 0, &raw mut report) }, INVALID);
        assert_eq!(unsafe { hypha_waitset_run(live.waitset, 0, null_mut()) }, INVALID);
    }
    hypha_waitset_request_stop => |_live| {
        assert_eq!(unsafe { hypha_waitset_request_stop(null_mut()) }, INVALID);
    }
    hypha_waitset_request_failure => |_live| {
        assert_eq!(unsafe { hypha_waitset_request_failure(null_mut()) }, INVALID);
    }
    hypha_waitset_timer_cancel => |_live| {
        // SAFETY: a null `WaitSet` is the refused argument; nothing is dereferenced.
        assert_eq!(unsafe { hypha_waitset_timer_cancel(null_mut(), 1) }, INVALID);
    }
    hypha_waitset_timer_reset => |_live| {
        // SAFETY: as for `hypha_waitset_timer_cancel`.
        assert_eq!(unsafe { hypha_waitset_timer_reset(null_mut(), 1) }, INVALID);
    }
    hypha_waitset_timer_state => |live| {
        let mut state = HyphaTimerState { canceled: false, period_nanos: 0, until_due_nanos: 0 };
        // SAFETY: a null `WaitSet` is the refused argument; `state` is a writable local.
        assert_eq!(unsafe { hypha_waitset_timer_state(null(), 1, &raw mut state) }, INVALID);
        // SAFETY: the `WaitSet` is live; a null `out` is the refused argument.
        assert_eq!(unsafe { hypha_waitset_timer_state(live.waitset, 1, null_mut()) }, INVALID);
    }
};

#[test]
fn every_row_refuses_its_invalid_argument() {
    let live = Live::new();
    for row in ROWS {
        eprintln!("{}", row.name);
        (row.check)(&live);
    }
}

/// The functions `hypha_ffi.h` declares: a line that starts with a return type and
/// contains `hypha_<name>(`.
fn header_functions() -> Vec<String> {
    let header = concat!(env!("CARGO_MANIFEST_DIR"), "/include/hypha/hypha_ffi.h");
    let text = std::fs::read_to_string(header).unwrap_or_default();
    let mut names = Vec::new();
    for line in text.lines() {
        let Some(first) = line.chars().next() else { continue };
        if !first.is_ascii_alphabetic() || line.starts_with("typedef") {
            continue;
        }
        let Some(start) = line.find("hypha_") else { continue };
        let name: String = line
            .get(start..)
            .unwrap_or_default()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if line.get(start.saturating_add(name.len())..).unwrap_or_default().starts_with('(') {
            names.push(name);
        }
    }
    names
}

#[test]
fn every_header_function_has_a_row() {
    let declared = header_functions();
    assert!(declared.len() >= 80, "the header parse found {} functions", declared.len());
    let mut known: Vec<&str> = ROWS.iter().map(|row| row.name).collect();
    known.extend(NO_PREAMBLE.iter().map(|(name, _)| *name));
    let missing: Vec<&String> =
        declared.iter().filter(|name| !known.contains(&name.as_str())).collect();
    assert!(missing.is_empty(), "functions without a row: {missing:?}");
    let stale: Vec<&&str> =
        known.iter().filter(|name| !declared.contains(&(**name).to_owned())).collect();
    assert!(stale.is_empty(), "rows without a header function: {stale:?}");
    let mut sorted = known.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), known.len(), "a function has two rows");
}

#[test]
fn the_header_revision_is_the_library_revision() {
    let header = concat!(env!("CARGO_MANIFEST_DIR"), "/include/hypha/hypha_ffi.h");
    let text = std::fs::read_to_string(header).unwrap_or_default();
    let defined = text
        .lines()
        .find_map(|line| line.strip_prefix("#define HYPHA_ABI_REVISION "))
        .and_then(|value| value.trim().parse::<u32>().ok());
    assert_eq!(defined, Some(hypha_abi_revision()));
}
