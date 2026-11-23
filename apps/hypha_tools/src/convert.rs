//! The core of `hypha convert`: a ROS 2 bag, in rosbag2's SQLite storage, into a hypha
//! MCAP recording that `hypha play` replays.
//!
//! Each topic's CDR messages become ROS 1 serialization, through the ROS 1 definition of
//! its type found in the message paths (`msg_library`). The recording carries each
//! type's full definition and MD5, as `hypha record` writes them, and each message's log
//! and publish time is the bag's receive time. A topic whose type has no definition, or
//! that is not CDR, is left out and listed in the report; one named in `topics` makes the
//! conversion fail instead.

use std::collections::BTreeMap;
use std::path::PathBuf;

use hypha_node::sdk::message::{DecodeOwned as _, EncodeInto as _};
use hypha_node::sdk::msgdef::spec::TypeName;
use hypha_node::sdk::msgs::sensor_msgs::PointCloud2;

use crate::cdr;
use crate::msg_library::{MessageLibrary, Resolved, ros1_type_of};
use crate::point_layout::xyzi_to_xyzirc;
use crate::recorder::{McapCompression, RecordSample, Recorder};
use crate::ros2_bag::{BagMessage, BagTopic, Ros2Bag};

/// What to convert, and how.
#[derive(Debug, Clone)]
pub struct ConvertOptions {
    /// The rosbag2 SQLite file, `.db3`.
    pub bag: PathBuf,
    /// The MCAP file to write.
    pub output: PathBuf,
    /// The directories of ROS 1 definitions, searched in order.
    pub message_paths: Vec<PathBuf>,
    /// The topics to convert; empty converts every topic that has a definition.
    pub topics: Vec<String>,
    /// The `sensor_msgs/PointCloud2` topics whose XYZI clouds become XYZIRC.
    pub xyzirc: Vec<String>,
    /// The chunk compression of the recording.
    pub compression: McapCompression,
}

/// One converted topic.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertedTopic {
    /// The topic name.
    pub topic: String,
    /// Its ROS 1 type, `package/Name`.
    pub data_type: String,
    /// How many messages the recording holds of it.
    pub messages: u64,
    /// For a topic rewritten into XYZIRC, the highest intensity of its clouds as
    /// recorded, before rounding to 0 to 255.
    pub highest_intensity: Option<f32>,
}

/// One topic left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedTopic {
    /// The topic name.
    pub topic: String,
    /// Its ROS 2 type.
    pub ros2_type: String,
    /// Why it was left out.
    pub reason: String,
}

/// What a conversion did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConvertReport {
    /// The converted topics, in the bag's order.
    pub converted: Vec<ConvertedTopic>,
    /// The topics left out.
    pub skipped: Vec<SkippedTopic>,
}

/// How one topic is converted.
struct TopicPlan {
    topic: String,
    root: TypeName,
    resolved: Resolved,
    xyzirc: bool,
    messages: u64,
    highest_intensity: Option<f32>,
}

/// Convert the bag of `options` into its recording.
///
/// # Errors
/// A message that names the topic and the message when the bag does not read, a topic
/// named in the options cannot be converted, a message does not transcode, or the
/// recording cannot be written.
pub fn convert(options: &ConvertOptions) -> Result<ConvertReport, String> {
    let bag = Ros2Bag::open(&options.bag)?;
    let topics = bag.topics()?;
    check_named_topics(options, &topics)?;
    let mut library = MessageLibrary::new(options.message_paths.clone());
    let mut report = ConvertReport::default();
    let mut plans = plan_topics(options, &topics, &mut library, &mut report)?;
    let mut recorder = Recorder::with_compression(&options.output, options.compression);
    if !recorder.is_open() {
        return Err(format!("{}: {}", options.output.display(), recorder.open_error()));
    }
    bag.for_each_message(|message| {
        let Some(plan) = plans.get_mut(&message.topic_id) else {
            return Ok(());
        };
        record_one(&mut recorder, &library, plan, message)
    })?;
    recorder.finish().map_err(|error| format!("{}: {error}", options.output.display()))?;
    report.converted = topics
        .iter()
        .filter_map(|topic| plans.get(&topic.id))
        .map(|plan| ConvertedTopic {
            topic: plan.topic.clone(),
            data_type: plan.resolved.type_name.clone(),
            messages: plan.messages,
            highest_intensity: plan.highest_intensity,
        })
        .collect();
    Ok(report)
}

/// Refuse a topic of the options that the bag does not hold.
fn check_named_topics(options: &ConvertOptions, topics: &[BagTopic]) -> Result<(), String> {
    let named = options.topics.iter().chain(&options.xyzirc);
    for name in named {
        if !topics.iter().any(|topic| &topic.name == name) {
            return Err(format!("the bag has no topic '{name}'"));
        }
    }
    let unconverted = options
        .xyzirc
        .iter()
        .find(|name| !options.topics.is_empty() && !options.topics.contains(name));
    if let Some(name) = unconverted {
        return Err(format!("--xyzirc '{name}' is not among the topics to convert"));
    }
    Ok(())
}

/// The plan of every topic to convert, by topic id; the others go into `report`.
fn plan_topics(
    options: &ConvertOptions,
    topics: &[BagTopic],
    library: &mut MessageLibrary,
    report: &mut ConvertReport,
) -> Result<BTreeMap<i64, TopicPlan>, String> {
    let mut plans = BTreeMap::new();
    for topic in topics {
        let named = options.topics.contains(&topic.name);
        if !options.topics.is_empty() && !named {
            continue;
        }
        match plan_topic(options, topic, library) {
            Ok(plan) => {
                plans.insert(topic.id, plan);
            }
            Err(reason) if named || options.xyzirc.contains(&topic.name) => {
                return Err(format!("topic '{}': {reason}", topic.name));
            }
            Err(reason) => report.skipped.push(SkippedTopic {
                topic: topic.name.clone(),
                ros2_type: topic.ros2_type.clone(),
                reason,
            }),
        }
    }
    Ok(plans)
}

fn plan_topic(
    options: &ConvertOptions,
    topic: &BagTopic,
    library: &mut MessageLibrary,
) -> Result<TopicPlan, String> {
    if topic.serialization_format != "cdr" {
        return Err(format!("serialized as '{}', not CDR", topic.serialization_format));
    }
    let root = ros1_type_of(&topic.ros2_type)
        .ok_or_else(|| format!("'{}' is not a message type", topic.ros2_type))?;
    let resolved = library.resolve(&root)?;
    let xyzirc = options.xyzirc.contains(&topic.name);
    if xyzirc && resolved.type_name != "sensor_msgs/PointCloud2" {
        return Err(format!("--xyzirc needs sensor_msgs/PointCloud2, not {}", resolved.type_name));
    }
    Ok(TopicPlan {
        topic: topic.name.clone(),
        root,
        resolved,
        xyzirc,
        messages: 0,
        highest_intensity: None,
    })
}

/// Transcode one message of `plan` and write it.
fn record_one(
    recorder: &mut Recorder,
    library: &MessageLibrary,
    plan: &mut TopicPlan,
    message: BagMessage<'_>,
) -> Result<(), String> {
    let at =
        |reason: String| format!("topic '{}', message {}: {reason}", plan.topic, plan.messages);
    let time = u64::try_from(message.timestamp_nanos)
        .map_err(|_| at(format!("receive time {} ns before 1970", message.timestamp_nanos)))?;
    // A header's sequence number: the message's index on its topic.
    let sequence = u32::try_from(plan.messages).unwrap_or(u32::MAX);
    let mut bytes = cdr::transcode(library.specs(), &plan.root, message.data, sequence)
        .map_err(|error| at(error.to_string()))?;
    if plan.xyzirc {
        let (rewritten, highest) = rewrite_xyzirc(&bytes).map_err(&at)?;
        bytes = rewritten;
        let before = plan.highest_intensity.unwrap_or(0.0);
        plan.highest_intensity = Some(before.max(highest));
    }
    let sample = RecordSample {
        topic: &plan.topic,
        data_type: &plan.resolved.type_name,
        md5: &plan.resolved.md5,
        definition: &plan.resolved.definition,
        bytes: &bytes,
        log_time_nanos: time,
        publish_time_nanos: time,
    };
    if !recorder.record(&sample) {
        return Err(at(String::from("the recording could not take it")));
    }
    plan.messages = plan.messages.saturating_add(1);
    Ok(())
}

/// The ROS 1 cloud `bytes`, rewritten from XYZI into XYZIRC, and its highest intensity.
fn rewrite_xyzirc(bytes: &[u8]) -> Result<(Vec<u8>, f32), String> {
    let mut cloud = PointCloud2::decode_from(bytes).map_err(|error| error.to_string())?;
    let highest = xyzi_to_xyzirc(&mut cloud)?;
    let mut out = vec![0; cloud.encoded_len()];
    let written = cloud.encode_into(&mut out).map_err(|error| error.to_string())?;
    out.truncate(written);
    Ok((out, highest))
}
