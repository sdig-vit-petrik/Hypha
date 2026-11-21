//! `HYPHA_USE_SIM_TIME` is the only switch of simulated time. Every case holds [`SERIAL`].

use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

mod common;

use hypha_ffi::*;

const VARIABLE: &str = "HYPHA_USE_SIM_TIME";

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Holds the process-wide state of one case and restores it on drop.
struct Case {
    _serial: MutexGuard<'static, ()>,
}

impl Case {
    fn new(environment: Option<&str>) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        match environment {
            Some(value) => hypha_testkit::set_env(VARIABLE, value),
            None => hypha_testkit::remove_env(VARIABLE),
        }
        Self { _serial: serial }
    }
}

/// Create a node, optionally with a parameter file. Returns the status and whether
/// the process clock follows simulated time.
fn create_node(parameter_text: Option<&str>) -> (HyphaStatus, bool) {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = CString::new(format!("sim_env_{}_{id}", std::process::id())).unwrap_or_default();
    let file = parameter_text.map(|text| {
        let path = std::env::temp_dir().join(format!("sim_env_{}_{id}.toml", std::process::id()));
        std::fs::write(&path, text).unwrap_or_default();
        path
    });
    let file_text = file.as_ref().and_then(|path| path.to_str()).unwrap_or("");
    let mut node = ptr::null_mut();
    let status = unsafe {
        hypha_node_create(
            name.as_ptr(),
            name.as_bytes().len(),
            file_text.as_ptr().cast(),
            file_text.len(),
            ptr::null(),
            &raw mut node,
        )
    };
    let simulated = hypha_clock_is_sim_time() != 0;
    if let Some(path) = file {
        std::fs::remove_file(path).unwrap_or_default();
    }
    unsafe { hypha_node_destroy(node) };
    hypha_clock_disable_sim_time();
    (status, simulated)
}

impl Drop for Case {
    fn drop(&mut self) {
        hypha_testkit::remove_env(VARIABLE);
        hypha_clock_disable_sim_time();
    }
}

#[test]
fn hypha_variable_zero_false_or_unset_keeps_host_time() {
    {
        let _case = Case::new(None);
        assert_eq!(create_node(None), (HyphaStatus::Ok, false), "unset");
    }
    for value in ["0", "false"] {
        let _case = Case::new(Some(value));
        assert_eq!(create_node(None), (HyphaStatus::Ok, false), "value {value}");
    }
}

#[test]
fn hypha_variable_one_and_true_switch_simulated_time_on() {
    for value in ["1", "true"] {
        let _case = Case::new(Some(value));
        assert_eq!(create_node(None), (HyphaStatus::Ok, true), "value {value}");
    }
}

#[test]
fn a_node_with_a_parameter_file_follows_the_variable() {
    let _case = Case::new(Some("1"));
    assert_eq!(create_node(Some("[node]\nname = \"sim_env\"\n")), (HyphaStatus::Ok, true));
}

#[test]
fn the_use_sim_time_key_does_not_switch_simulated_time() {
    {
        let _case = Case::new(None);
        assert_eq!(create_node(Some("use_sim_time = true\n")), (HyphaStatus::Ok, false));
    }
    let _case = Case::new(Some("1"));
    assert_eq!(create_node(Some("use_sim_time = false\n")), (HyphaStatus::Ok, true));
}

#[test]
fn a_variable_with_another_value_stops_node_creation() {
    for value in ["", "yes", "2", "TRUE", " 1"] {
        let _case = Case::new(Some(value));
        assert_eq!(create_node(None), (HyphaStatus::InvalidArgument, false), "value '{value}'");
    }
}

#[test]
fn a_variable_with_another_value_stops_node_creation_also_when_the_file_sets_the_key() {
    let _case = Case::new(Some("maybe"));
    assert_eq!(create_node(Some("use_sim_time = false\n")), (HyphaStatus::InvalidArgument, false));
}

#[test]
fn a_use_sim_time_value_of_another_type_does_not_stop_node_creation() {
    let _case = Case::new(None);
    for text in ["use_sim_time = 1\n", "use_sim_time = \"true\"\n"] {
        assert_eq!(create_node(Some(text)), (HyphaStatus::Ok, false), "{text}");
    }
}

extern "C" fn stop_cycle_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut std::ffi::c_void,
) -> HyphaCallbackResult {
    let counter = unsafe { &*context.cast::<AtomicU32>() };
    counter.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: hypha_ffi::waitset::HYPHA_ACTION_STOP, status: 0 }
}

unsafe fn publish_clock_sample(publisher: *mut HyphaPublisher, secs: u32, nsecs: u32) {
    let mut payload = [0_u8; 8];
    payload[..4].copy_from_slice(&secs.to_le_bytes());
    payload[4..].copy_from_slice(&nsecs.to_le_bytes());
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publisher_loan_frame(publisher, payload.len(), &raw mut loan, &raw mut bytes)
        },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(payload.as_ptr(), bytes, payload.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

/// Publish the first clock value and run once, so the cycle takes its phase from it.
unsafe fn take_the_phase(clock_pub: *mut HyphaPublisher, waitset: *mut HyphaWaitSet) {
    unsafe { publish_clock_sample(clock_pub, 1, 0) };
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 120_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
}

fn clock_cycle_publish_and_wait() -> (Duration, AtomicU32) {
    let _case = Case::new(Some("1"));
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = CString::new(format!("sim_cycle_{}_{id}", std::process::id())).unwrap_or_default();
    let mut node = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_node_create(
                name.as_ptr(),
                name.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        },
        HyphaStatus::Ok
    );

    let topic = CString::new("/clock").unwrap_or_default();
    let data_type = CString::new("rosgraph_msgs/Clock").unwrap_or_default();
    let md5 = CString::new("a9c97c1d230cfc112e270351a944ee47").unwrap_or_default();
    let pub_qos = HyphaPublisherOptions {
        latch: 0,
        loaned_samples: 0,
        qos: crate::common::qos_for_topic("/clock"),
    };
    let mut clock_pub = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                data_type.as_ptr(),
                data_type.as_bytes().len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                c"time clock".as_ptr(),
                "time clock".len(),
                &raw const pub_qos,
                &raw mut clock_pub,
            )
        },
        HyphaStatus::Ok
    );

    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    let steps = std::sync::atomic::AtomicU32::new(0);
    let cycle_name = CString::new("fast_sim_cycle").unwrap_or_default();
    let mut cycle = ptr::null_mut();
    // This cycle is dormant until the /clock sample advances framework time.
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                cycle_name.as_ptr(),
                cycle_name.as_bytes().len(),
                20_000_000,
                Some(stop_cycle_step),
                (&raw const steps).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    unsafe { take_the_phase(clock_pub, waitset) };
    assert_eq!(steps.load(Ordering::SeqCst), 0, "the first clock value runs no step");

    // The next clock value passes the first deadline, 20 ms after the phase.
    let waitset_address = waitset as usize;
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    let runner = std::thread::spawn(move || {
        let waitset = waitset_address as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        assert!(ready_tx.send(()).is_ok(), "waitset runner is ready");
        let status = unsafe { hypha_waitset_run(waitset, 250_000_000, &raw mut report) };
        (status, report)
    });
    assert!(ready_rx.recv().is_ok(), "waitset runner started");
    std::thread::sleep(Duration::from_millis(30));
    let published = Instant::now();
    unsafe { publish_clock_sample(clock_pub, 1, 50_000_000) };
    let joined = runner.join();
    assert!(joined.is_ok(), "waitset runner panicked");
    let Ok((status, report)) = joined else {
        return (Duration::MAX, steps);
    };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.status, HyphaStatus::Ok as i32);
    assert_eq!(steps.load(Ordering::SeqCst), 1, "the simulated cycle did not run once");
    let elapsed = published.elapsed();

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(clock_pub);
        hypha_node_destroy(node);
    }
    (elapsed, steps)
}

#[test]
fn a_clock_publisher_wakes_a_sim_time_cycle_promptly() {
    let (elapsed, _steps) = clock_cycle_publish_and_wait();
    assert!(elapsed < Duration::from_millis(60), "clock notification took {elapsed:?}");
}

#[test]
fn destroying_a_node_removes_its_iceoryx2_node_directory() {
    let _case = Case::new(Some("1"));
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let node_name = format!("clean_teardown_{}_{id}", std::process::id());
    let c_name = CString::new(node_name.clone()).unwrap_or_default();
    let mut node = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_node_create(
                c_name.as_ptr(),
                c_name.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        },
        HyphaStatus::Ok
    );
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    unsafe { hypha_node_destroy(node) };

    let nodes_dir = std::path::Path::new("/tmp/iceoryx2/nodes");
    if let Ok(entries) = std::fs::read_dir(nodes_dir) {
        for entry in entries.flatten() {
            let details_path = entry.path().join("node.details");
            if let Ok(contents) = std::fs::read_to_string(&details_path) {
                assert!(
                    !contents.contains(&node_name),
                    "node directory {} was left behind after hypha_node_destroy",
                    entry.path().display()
                );
            }
        }
    }
}

struct SimNode(*mut HyphaNode);

impl SimNode {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let name =
            CString::new(format!("sim_keep_{}_{id}", std::process::id())).unwrap_or_default();
        let mut node = ptr::null_mut();
        let status = unsafe {
            hypha_node_create(
                name.as_ptr(),
                name.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        Self(node)
    }
}

impl Drop for SimNode {
    fn drop(&mut self) {
        unsafe { hypha_node_destroy(self.0) };
    }
}

#[test]
fn creating_a_second_sim_node_keeps_the_clock_reading() {
    let _case = Case::new(Some("1"));
    let _first = SimNode::new();
    assert_eq!(hypha_clock_set_sim_time(5_000_000_000), HyphaStatus::Ok as i32);
    let _second = SimNode::new();
    assert_eq!(hypha_clock_now_nanos(), 5_000_000_000, "the second node did not reset the clock");
}

#[test]
fn destroying_a_sim_node_keeps_simulated_time_for_the_survivors() {
    let _case = Case::new(Some("1"));
    let first = SimNode::new();
    let _second = SimNode::new();
    drop(first);
    assert_ne!(hypha_clock_is_sim_time(), 0, "the surviving node is still on simulated time");
}
