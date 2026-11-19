//! The TF input of a cycle through the C ABI.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::float_cmp,
    clippy::indexing_slicing
)]

mod common;
use crate::common::replay::{
    MILLISECOND, SECOND, SimulatedClock, create_waitset, open_subscriber, run_for, set_clock,
};
use crate::common::{c_text, create_node, create_node_with_file, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, PoisonError};

use hypha_core::message::EncodeInto as _;
use hypha_ffi::waitset::*;
use hypha_ffi::*;
use hypha_msgs::geometry_msgs::{Quaternion, Transform, TransformStamped, Vector3};
use hypha_msgs::std_msgs::Header;
use hypha_msgs::tf2_msgs::TFMessage;
use hypha_wire::ros_time::RosTime;

/// The simulated clock is process-wide, so host-time cases hold its lock too.
use crate::common::replay::SERIAL as HOST;

const PERIOD_NANOS: i64 = 20_000_000;
const EMPTY: HyphaTransform =
    HyphaTransform { translation: [0.0; 3], rotation_xyzw: [0.0; 4], stamp_nanos: 0 };

/// One lookup the step makes: target, source, time (`LATEST`: the latest common time).
type Query = (&'static str, &'static str, Option<i64>);
const LATEST: Option<i64> = None;

/// One step as recorded: its deadline, and (status, result, transform) per query.
#[derive(Debug, Clone)]
struct Seen {
    scheduled: i64,
    answers: Vec<(HyphaStatus, u32, HyphaTransform)>,
    /// The status of a lookup that asks for the latest time and names a time too.
    latest_and_time: HyphaStatus,
}

struct Probe {
    queries: Mutex<Vec<Query>>,
    seen: Mutex<Vec<Seen>>,
    index: u32,
}

impl Probe {
    fn last(&self) -> Seen {
        self.seen.lock().unwrap().last().cloned().expect("a step ran")
    }
}

extern "C" fn lookup_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let probe = unsafe { &*context.cast::<Probe>() };
    let time = unsafe { *time };
    let queries = probe.queries.lock().unwrap().clone();
    let answers = queries
        .iter()
        .map(|(target, source, at)| {
            let mut result = u32::MAX;
            let mut out = EMPTY;
            let status = unsafe {
                hypha_cycle_image_lookup_transform(
                    image,
                    probe.index,
                    target.as_ptr().cast(),
                    target.len(),
                    source.as_ptr().cast(),
                    source.len(),
                    at.unwrap_or(0),
                    u32::from(at.is_none()),
                    &raw mut result,
                    &raw mut out,
                )
            };
            (status, result, out)
        })
        .collect();
    let (mut result, mut out) = (u32::MAX, EMPTY);
    let latest_and_time = unsafe {
        hypha_cycle_image_lookup_transform(
            image,
            probe.index,
            "a".as_ptr().cast(),
            1,
            "a".as_ptr().cast(),
            1,
            SECOND,
            1,
            &raw mut result,
            &raw mut out,
        )
    };
    probe.seen.lock().unwrap().push(Seen {
        scheduled: time.scheduled_nanos,
        answers,
        latest_and_time,
    });
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A `TFMessage` with one transform of `child` in `parent`, translated by `xyz`.
fn tf_message(parent: &str, child: &str, stamp_nanos: i64, xyz: [f64; 3]) -> Vec<u8> {
    let stamp = u64::try_from(stamp_nanos).unwrap();
    let stamp = RosTime::new(
        u32::try_from(stamp / 1_000_000_000).unwrap(),
        u32::try_from(stamp % 1_000_000_000).unwrap(),
    );
    tf_message_at(parent, child, stamp, xyz)
}

/// As [`tf_message`], with the given header stamp.
fn tf_message_at(parent: &str, child: &str, stamp: RosTime, xyz: [f64; 3]) -> Vec<u8> {
    let message = TFMessage {
        transforms: vec![TransformStamped {
            header: Header { seq: 0, stamp, frame_id: parent.into() },
            child_frame_id: child.into(),
            transform: Transform {
                translation: Vector3 { x: xyz[0], y: xyz[1], z: xyz[2] },
                rotation: Quaternion { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
            },
        }],
    };
    let mut payload = vec![0_u8; message.encoded_len()];
    message.encode_into(&mut payload).expect("a TFMessage encodes");
    payload
}

/// Send `payload`, stamped with framework time `sent_nanos` when it is `Some`.
unsafe fn send(publisher: *mut HyphaPublisher, payload: &[u8], sent_nanos: Option<i64>) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publisher_loan_frame(publisher, payload.len(), &raw mut loan, &raw mut bytes)
        },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(payload.as_ptr(), bytes, payload.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, sent_nanos.unwrap_or(-1)) }, HyphaStatus::Ok);
}

struct TfRig {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    tf: *mut HyphaPublisher,
    tf_static: *mut HyphaPublisher,
    sources: [*mut HyphaInputSource; 2],
    cycle: *mut HyphaCycle,
    probe: Box<Probe>,
}

fn tf_publisher(node: *mut HyphaNode, topic: &str, latch: bool) -> *mut HyphaPublisher {
    let topic = c_text(topic);
    let data_type = c_text(TFMessage::TYPE_NAME);
    let md5 = c_text(TFMessage::SCHEMA_HASH);
    let definition = c_text("");
    // A declared policy, so a subscriber may read the transforms `depth` 32 deep.
    let qos = HyphaPublisherOptions {
        latch: i32::from(latch),
        loaned_samples: 0,
        qos: crate::common::test_qos(),
    };
    let mut publisher = ptr::null_mut();
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
                definition.as_ptr(),
                0,
                &raw const qos,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );
    publisher
}

fn tf_source(
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    topic: &str,
    depth: usize,
) -> *mut HyphaInputSource {
    let subscriber = unsafe { open_subscriber(node, topic, TFMessage::SCHEMA_HASH, depth) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    source
}

impl TfRig {
    /// `tf_depth` is the `/tf` queue; the cache holds 10 s.
    fn new(label: &str, tf_depth: usize) -> Self {
        Self::with_budget(label, tf_depth, None)
    }

    /// As [`TfRig::new`], with a cycle arrival budget from the `[cycle]` table.
    fn with_budget(label: &str, tf_depth: usize, budget: Option<usize>) -> Self {
        let cycle_name = unique("tf_cycle");
        let node = match budget {
            None => create_node(&unique(label)),
            Some(budget) => {
                let path = std::env::temp_dir().join(format!("{}.toml", unique(label)));
                std::fs::write(&path, format!("[cycle]\narrival_budget = {budget}\n"))
                    .expect("parameter file");
                let node = create_node_with_file(&unique(label), Some(&path));
                std::fs::remove_file(path).expect("remove parameter file");
                node
            }
        };
        let waitset = unsafe { create_waitset(node) };
        let tf_topic = format!("/{}", unique("tf"));
        let static_topic = format!("/{}", unique("tf_static"));
        let tf = tf_publisher(node, &tf_topic, false);
        let tf_static = tf_publisher(node, &static_topic, true);
        let sources = [
            tf_source(node, waitset, &tf_topic, tf_depth),
            tf_source(node, waitset, &static_topic, 32),
        ];
        let probe = Box::new(Probe {
            queries: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            index: 0,
        });
        let name = c_text(&cycle_name);
        let mut cycle = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_cycle_create(
                    waitset,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    PERIOD_NANOS as u64,
                    Some(lookup_step),
                    (&raw const *probe).cast_mut().cast(),
                    None,
                    &raw mut cycle,
                )
            },
            HyphaStatus::Ok
        );
        let mut index = u32::MAX;
        assert_eq!(
            unsafe {
                hypha_cycle_bind_transforms(
                    cycle,
                    sources[0],
                    sources[1],
                    10 * SECOND as u64,
                    &raw mut index,
                )
            },
            HyphaStatus::Ok
        );
        assert_eq!(index, 0, "the first TF input of a cycle");
        if budget.is_some() {
            assert_eq!(unsafe { hypha_cycle_set_class_based(cycle, 1) }, HyphaStatus::Ok);
        }
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        Self { node, waitset, tf, tf_static, sources, cycle, probe }
    }

    fn ask(&self, queries: &[Query]) {
        *self.probe.queries.lock().unwrap() = queries.to_vec();
    }

    fn settle(&self) {
        run_for(self.waitset, 5_000_000);
    }

    fn settle_status(&self) -> i32 {
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        let status = unsafe { hypha_waitset_run(self.waitset, 5_000_000, &raw mut report) };
        assert_eq!(status, HyphaStatus::Ok);
        report.status
    }
}

impl Drop for TfRig {
    fn drop(&mut self) {
        assert_eq!(unsafe { hypha_cycle_destroy(self.cycle) }, HyphaStatus::Ok);
        for source in self.sources {
            assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
        }
        unsafe {
            hypha_publisher_destroy(self.tf);
            hypha_publisher_destroy(self.tf_static);
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn a_step_finds_a_published_transform() {
    let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = TfRig::new("tf_found", 32);
    rig.ask(&[("world", "base", LATEST), ("/world", "/base", Some(5 * SECOND))]);
    unsafe { send(rig.tf, &tf_message("world", "base", 5 * SECOND, [1.0, 2.0, 3.0]), None) };
    run_for(rig.waitset, 100_000_000);
    let last = rig.probe.last();
    for (status, result, transform) in last.answers {
        assert_eq!((status, result), (HyphaStatus::Ok, HYPHA_TF_FOUND));
        assert_eq!(transform.translation, [1.0, 2.0, 3.0]);
        assert_eq!(transform.rotation_xyzw, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(transform.stamp_nanos, 5 * SECOND);
    }
    assert_eq!(last.latest_and_time, HyphaStatus::InvalidArgument, "latest names no time");
}

#[test]
fn a_static_transform_is_found_at_any_time() {
    let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = TfRig::new("tf_static", 32);
    rig.ask(&[("base", "camera", LATEST), ("base", "camera", Some(SECOND))]);
    unsafe { send(rig.tf_static, &tf_message("base", "camera", 0, [0.0, 0.0, 0.5]), None) };
    run_for(rig.waitset, 100_000_000);
    let last = rig.probe.last();
    let found: Vec<(u32, i64)> = last.answers.iter().map(|a| (a.1, a.2.stamp_nanos)).collect();
    assert_eq!(found, [(HYPHA_TF_FOUND, 0), (HYPHA_TF_FOUND, SECOND)]);
    assert!(last.answers.iter().all(|a| a.2.translation == [0.0, 0.0, 0.5]));
}

#[test]
fn an_unknown_frame_reports_unknown() {
    let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = TfRig::new("tf_unknown", 32);
    rig.ask(&[("world", "nowhere", LATEST), ("", "base", LATEST)]);
    unsafe { send(rig.tf, &tf_message("world", "base", SECOND, [1.0, 0.0, 0.0]), None) };
    run_for(rig.waitset, 100_000_000);
    let answers: Vec<(HyphaStatus, u32)> =
        rig.probe.last().answers.iter().map(|a| (a.0, a.1)).collect();
    assert_eq!(
        answers,
        [(HyphaStatus::Ok, HYPHA_TF_UNKNOWN_FRAME), (HyphaStatus::Ok, HYPHA_TF_INVALID_FRAME)]
    );
}

#[test]
fn bind_transforms_refuses_a_zero_cache() {
    let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("tf_refuse"));
    let waitset = unsafe { create_waitset(node) };
    let tf = tf_source(node, waitset, &format!("/{}", unique("tf")), 8);
    let tf_static = tf_source(node, waitset, &format!("/{}", unique("tf_static")), 8);
    let probe = Probe { queries: Mutex::new(Vec::new()), seen: Mutex::new(Vec::new()), index: 0 };
    let name = c_text("tf_refuse_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                PERIOD_NANOS as u64,
                Some(lookup_step),
                (&raw const probe).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let mut index = 7_u32;
    let bind = |tf, tf_static, cache, out| unsafe {
        hypha_cycle_bind_transforms(cycle, tf, tf_static, cache, out)
    };
    assert_eq!(bind(tf, tf_static, 0, &raw mut index), HyphaStatus::InvalidArgument);
    assert_eq!(index, 0, "a refusal writes index zero");
    assert_eq!(bind(tf, tf, SECOND as u64, &raw mut index), HyphaStatus::InvalidArgument);
    assert_eq!(
        bind(ptr::null_mut(), tf_static, SECOND as u64, &raw mut index),
        HyphaStatus::InvalidArgument
    );
    assert_eq!(bind(tf, tf_static, SECOND as u64, ptr::null_mut()), HyphaStatus::InvalidArgument);
    assert_eq!(
        unsafe {
            hypha_cycle_bind_transforms(
                ptr::null_mut(),
                tf,
                tf_static,
                SECOND as u64,
                &raw mut index,
            )
        },
        HyphaStatus::InvalidArgument
    );
    let mut result = 0_u32;
    let mut out = EMPTY;
    assert_eq!(
        unsafe {
            hypha_cycle_image_lookup_transform(
                ptr::null(),
                0,
                "a".as_ptr().cast(),
                1,
                "b".as_ptr().cast(),
                1,
                0,
                0,
                &raw mut result,
                &raw mut out,
            )
        },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(tf) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(tf_static) }, HyphaStatus::Ok);
    unsafe {
        hypha_node_destroy(node);
    }
}

#[test]
fn the_replay_cut_hides_newer_transforms() {
    let _clock = SimulatedClock::start(0);
    let rig = TfRig::new("tf_cut", 32);
    rig.ask(&[("world", "base", LATEST)]);
    let deadline = 40 * MILLISECOND;
    unsafe {
        let before = tf_message("world", "base", deadline - MILLISECOND, [1.0, 0.0, 0.0]);
        send(rig.tf, &before, Some(deadline - MILLISECOND));
        let at = tf_message("world", "base", deadline, [2.0, 0.0, 0.0]);
        send(rig.tf, &at, Some(deadline));
    }
    set_clock(deadline);
    rig.settle();
    let seen = rig.probe.seen.lock().unwrap().clone();
    let step = seen.iter().find(|step| step.scheduled == deadline).expect("the 40 ms step ran");
    let (status, result, transform) = step.answers[0];
    assert_eq!((status, result), (HyphaStatus::Ok, HYPHA_TF_FOUND));
    assert_eq!(
        transform.stamp_nanos,
        deadline - MILLISECOND,
        "sent at the deadline is not admitted"
    );
    assert_eq!(transform.translation[0], 1.0);

    set_clock(deadline + PERIOD_NANOS);
    rig.settle();
    let (_, _, transform) = rig.probe.last().answers[0];
    assert_eq!(transform.stamp_nanos, deadline, "the next step admits it");
}

/// A replay step must take every transform sent before its time; when more than one
/// step's share is pending, the step fails.
#[test]
fn a_replay_step_with_more_transforms_than_it_takes_fails() {
    let _clock = SimulatedClock::start(0);
    let rig = TfRig::new("tf_replay_burst", 32);
    let send_batch = |first: i64| {
        for index in 0..30 {
            let stamp = first + index * 500_000;
            unsafe { send(rig.tf, &tf_message("world", "base", stamp, [0.0; 3]), Some(stamp)) };
        }
    };
    // 10 samples before the 20 ms step, 20 after it.
    send_batch(15 * MILLISECOND);
    set_clock(PERIOD_NANOS);
    assert_eq!(rig.settle_status(), HyphaStatus::Ok as i32);
    // 30 more before the 40 ms step: 50 are pending there.
    send_batch(25 * MILLISECOND);
    set_clock(2 * PERIOD_NANOS);
    assert_eq!(
        rig.settle_status(),
        HyphaStatus::SequenceGap as i32,
        "the 40 ms step cannot take all 50 transforms"
    );
}

#[test]
fn a_tf_overflow_is_counted() {
    let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = TfRig::new("tf_overflow", 4);
    unsafe { send(rig.tf, &tf_message("world", "base", SECOND, [0.0; 3]), None) };
    run_for(rig.waitset, 50_000_000);
    for index in 0..10 {
        let stamp = 2 * SECOND + index * MILLISECOND;
        unsafe { send(rig.tf, &tf_message("world", "base", stamp, [0.0; 3]), None) };
    }
    run_for(rig.waitset, 100_000_000);
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(rig.cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let health = binding.health;
    assert!(health.overrun_total > 0, "the /tf burst lost samples: {health:?}");
}

/// The TF bindings take every pending transform, outside the cycle's arrival budget.
#[test]
fn the_arrival_budget_does_not_drop_transforms() {
    let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = TfRig::with_budget("tf_budget", 32, Some(1));
    rig.ask(&[("base", "sensor", LATEST), ("world", "base", LATEST)]);
    unsafe {
        send(rig.tf_static, &tf_message("base", "sensor", 0, [0.0, 0.0, 0.2]), None);
        for index in 1..=3 {
            send(rig.tf, &tf_message("world", "base", index * SECOND, [0.0; 3]), None);
        }
    }
    run_for(rig.waitset, 100_000_000);
    let answers: Vec<(u32, i64)> =
        rig.probe.last().answers.iter().map(|a| (a.1, a.2.stamp_nanos)).collect();
    assert_eq!(answers[0].0, HYPHA_TF_FOUND, "the static transform is kept: {answers:?}");
    assert_eq!(answers[1], (HYPHA_TF_FOUND, 3 * SECOND), "every /tf sample is taken");
    for binding in 0..2 {
        let mut view = HyphaCycleBindingView::default();
        assert_eq!(
            unsafe { hypha_cycle_binding(rig.cycle, binding, &raw mut view) },
            HyphaStatus::Ok
        );
        let health = view.health;
        assert_eq!(health.overrun_total, 0, "binding {binding} lost samples: {health:?}");
    }
}

#[test]
fn a_stamp_with_a_second_of_nanoseconds_is_refused() {
    let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = TfRig::new("tf_bad_nsecs", 32);
    rig.ask(&[("world", "base", LATEST), ("world", "good", LATEST)]);
    unsafe {
        send(
            rig.tf,
            &tf_message_at("world", "base", RosTime::new(1, 1_500_000_000), [0.0; 3]),
            None,
        );
        send(rig.tf, &tf_message("world", "good", SECOND, [0.0; 3]), None);
    }
    run_for(rig.waitset, 100_000_000);
    let results: Vec<u32> = rig.probe.last().answers.iter().map(|a| a.1).collect();
    assert_eq!(results, [HYPHA_TF_UNKNOWN_FRAME, HYPHA_TF_FOUND]);
}
