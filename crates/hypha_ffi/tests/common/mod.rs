//! Shared helpers for independent C ABI integration-test binaries.
// Each test binary uses only some helpers.
#![allow(dead_code)]

pub(crate) mod attach;
pub(crate) mod introspection;
pub(crate) mod receive;
pub(crate) mod replay;
pub(crate) mod supervision;

use std::os::raw::{c_char, c_void};
use std::sync::atomic::AtomicU64;

use hypha_ffi::{
    HyphaConfig, HyphaConfigValue, HyphaMetricCallback, HyphaNode, HyphaParamResult, HyphaStatus,
    HyphaSubscriber,
};
pub(crate) use hypha_testkit::unique_name as unique;

static NEXT_NAME: AtomicU64 = AtomicU64::new(0);

/// The service policy every test publisher declares: 32 publishers, 32 subscribers and
/// a queue depth of 32; the `default` policy admits 8 publishers and 16 subscribers.
pub(crate) const TEST_POLICY: &str = "test_deep";

/// The `HyphaQos` a test publisher passes: the test policy, with no payload bound.
pub(crate) fn test_qos() -> hypha_ffi::HyphaQos {
    hypha_ffi::HyphaQos {
        name: TEST_POLICY.as_ptr().cast(),
        name_len: TEST_POLICY.len(),
        max_publishers: 32,
        max_subscribers: 32,
        max_depth: 32,
        max_payload_dynamic: 0,
        initial_payload: 0,
    }
}

/// The `HyphaQos` a test publisher of `topic` passes; a framework topic declares nothing.
pub(crate) fn qos_for_topic(topic: &str) -> hypha_ffi::HyphaQos {
    let framework_topics = ["/clock", "/clock_progress"];
    if framework_topics.contains(&topic) { hypha_ffi::HyphaQos::default() } else { test_qos() }
}

/// Stage one message of a cycle step: reserve, copy and commit. A null reservation is a
/// dropped message. Returns the first status that is not `Ok`.
/// # Safety
/// `emission` must be valid for the current callback.
pub(crate) unsafe fn emit_message(
    emission: *mut hypha_ffi::HyphaCycleEmission,
    output: u32,
    bytes: &[u8],
) -> hypha_ffi::HyphaStatus {
    let mut payload = std::ptr::null_mut();
    let reserved = unsafe {
        hypha_ffi::hypha_cycle_emission_reserve(emission, output, bytes.len(), &raw mut payload)
    };
    if reserved != hypha_ffi::HyphaStatus::Ok {
        return reserved;
    }
    if payload.is_null() {
        return hypha_ffi::HyphaStatus::Ok;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), payload, bytes.len()) };
    unsafe { hypha_ffi::hypha_cycle_emission_commit(emission) }
}

pub(crate) fn create_node(name: &str) -> *mut hypha_ffi::HyphaNode {
    create_node_with_file(name, None)
}

#[allow(clippy::expect_used, reason = "an invalid test node name or path is a setup failure")]
pub(crate) fn create_node_with_file(
    name: &str,
    parameters: Option<&std::path::Path>,
) -> *mut hypha_ffi::HyphaNode {
    let name_c = c_text(name);
    let path_c = parameters
        .map(|file| std::ffi::CString::new(file.to_string_lossy().as_bytes()).expect("valid path"));
    let (path_ptr, path_len) = path_c
        .as_ref()
        .map_or((std::ptr::null(), 0), |path| (path.as_ptr(), path.as_bytes().len()));
    let mut node = std::ptr::null_mut();
    let status = unsafe {
        hypha_ffi::hypha_node_create(
            name_c.as_ptr(),
            name_c.as_bytes().len(),
            path_ptr,
            path_len,
            std::ptr::null(),
            &raw mut node,
        )
    };
    assert_eq!(status, hypha_ffi::HyphaStatus::Ok);
    node
}

/// Create a node on simulated time. `HYPHA_USE_SIM_TIME` is set for the call only;
/// the environment is process-wide, so the caller holds its binary's serial lock.
pub(crate) fn create_sim_node_with_file(
    name: &str,
    parameters: Option<&std::path::Path>,
) -> *mut hypha_ffi::HyphaNode {
    let _variable = SimTimeVariable::set();
    create_node_with_file(name, parameters)
}

/// Sets `HYPHA_USE_SIM_TIME=1` and restores the value it replaced on drop, also when
/// node creation panics.
struct SimTimeVariable {
    previous: Option<std::ffi::OsString>,
}

impl SimTimeVariable {
    const NAME: &'static str = "HYPHA_USE_SIM_TIME";

    fn set() -> Self {
        let previous = std::env::var_os(Self::NAME);
        hypha_testkit::set_env(Self::NAME, "1");
        Self { previous }
    }
}

impl Drop for SimTimeVariable {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => hypha_testkit::set_env(Self::NAME, value),
            None => hypha_testkit::remove_env(Self::NAME),
        }
    }
}

#[allow(clippy::expect_used, reason = "test setup text must be a valid C string")]
pub(crate) fn c_text(text: &str) -> std::ffi::CString {
    std::ffi::CString::new(text).expect("valid C string")
}

pub(crate) fn c_text_opt(text: &str) -> Option<std::ffi::CString> {
    std::ffi::CString::new(text).ok()
}

pub(crate) fn create_unique_node(label: &str) -> *mut hypha_ffi::HyphaNode {
    let name = unique(label);
    create_node(&name)
}

/// Collects the metrics one call emits. `user_data` points to the list.
extern "C" fn collect_metric(
    name: *const c_char,
    name_len: usize,
    value: u64,
    user_data: *mut c_void,
) {
    let list = unsafe { &mut *user_data.cast::<Vec<(String, u64)>>() };
    let bytes = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), name_len) };
    list.push((String::from_utf8_lossy(bytes).into_owned(), value));
}

pub(crate) fn collect_metrics(
    read: impl FnOnce(HyphaMetricCallback, *mut c_void) -> HyphaStatus,
) -> Result<Vec<(String, u64)>, HyphaStatus> {
    let mut list = Vec::new();
    let status = read(Some(collect_metric), (&raw mut list).cast());
    if status == HyphaStatus::Ok { Ok(list) } else { Err(status) }
}

/// Every metric of `node`, in emission order. Fails the test when the call fails.
#[allow(clippy::expect_used, reason = "a failed metrics read is a test setup failure")]
pub(crate) fn node_metrics(node: *const HyphaNode) -> Vec<(String, u64)> {
    collect_metrics(|callback, user_data| unsafe {
        hypha_ffi::hypha_node_metrics(node, callback, user_data)
    })
    .expect("hypha_node_metrics succeeds")
}

/// The value of the named metric of `node`, or `None` when the node does not emit that name.
pub(crate) fn node_metric(node: *const HyphaNode, name: &str) -> Option<u64> {
    lookup(&node_metrics(node), name)
}

/// The value of the named metric of `subscriber`, or `None` when it does not emit that name.
/// # Safety
/// `subscriber` must be a live handle from `hypha_subscribe_typed`.
pub(crate) unsafe fn subscriber_metric(
    subscriber: *const HyphaSubscriber,
    name: &str,
) -> Option<u64> {
    // SAFETY: the caller passes a live handle, which points to the runtime's subscriber.
    let endpoint = unsafe { subscriber.as_ref() }?;
    let mut list = Vec::new();
    hypha_node::safe::subscriber_metrics(endpoint, &mut |metric, value| {
        list.push((metric.to_owned(), value));
    });
    lookup(&list, name)
}

fn lookup(list: &[(String, u64)], name: &str) -> Option<u64> {
    list.iter().find(|(metric, _)| metric == name).map(|(_, value)| *value)
}

/// The `schema.*` metrics of one node.
pub(crate) struct SchemaCounts {
    pub(crate) invalid_records: u64,
    pub(crate) unavailable_lookups: u64,
    pub(crate) resource_limit_failures: u64,
}

#[allow(clippy::expect_used, reason = "a missing schema metric is a test setup failure")]
pub(crate) fn schema_counts(node: *const HyphaNode) -> SchemaCounts {
    let value = |name: &str| node_metric(node, name).expect("a schema metric");
    SchemaCounts {
        invalid_records: value("schema.invalid_records"),
        unavailable_lookups: value("schema.unavailable_lookups"),
        resource_limit_failures: value("schema.resource_limit_failures"),
    }
}

pub(crate) fn empty_config_value() -> HyphaConfigValue {
    HyphaConfigValue {
        kind: u32::MAX,
        bool_value: 0,
        int_value: 0,
        float_value: 0.0,
        text: std::ptr::null(),
        text_len: 0,
    }
}

pub(crate) fn config_value(
    config: *const HyphaConfig,
    name: &str,
) -> (HyphaParamResult, HyphaConfigValue) {
    let mut value = empty_config_value();
    let result = unsafe {
        hypha_ffi::hypha_config_value(config, name.as_ptr().cast(), name.len(), &raw mut value)
    };
    (result, value)
}

pub(crate) fn config_list_value(
    config: *const HyphaConfig,
    name: &str,
    index: usize,
    field: &str,
) -> (HyphaParamResult, HyphaConfigValue) {
    let mut value = empty_config_value();
    let result = unsafe {
        hypha_ffi::hypha_config_list_value(
            config,
            name.as_ptr().cast(),
            name.len(),
            index,
            field.as_ptr().cast(),
            field.len(),
            &raw mut value,
        )
    };
    (result, value)
}

/// The bytes that a TEXT value borrows, valid while the configuration lives.
pub(crate) fn stored_text(value: &HyphaConfigValue) -> &[u8] {
    if value.text.is_null() {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(value.text.cast::<u8>(), value.text_len) }
}
