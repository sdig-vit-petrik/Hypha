//! `hypha convert` -- turn a ROS 2 bag into a hypha MCAP recording.

use crate::cli::parse_cli;
use clap::Parser;
use hypha_tools::convert::{ConvertOptions, convert};
use hypha_tools::msg_library::installed_message_path;
use hypha_tools::recorder::McapCompression;
use std::path::PathBuf;
use std::process::ExitCode;

/// Turn a ROS 2 bag (rosbag2 SQLite, `.db3`) into an MCAP recording that `hypha play`
/// replays.
#[derive(Debug, Parser)]
#[command(
    name = "hypha convert",
    after_help = "Each topic's CDR messages become ROS 1 serialization through the ROS 1 \
                  definition of its type, looked up in the message paths: those of --msg-path \
                  in order, then the SDK's share/hypha/msg. A std_msgs/Header gets the \
                  message's index on its topic as its seq. A topic whose type has no \
                  definition is left out and listed; one named with --topic fails the \
                  conversion instead. Uncompress a .db3.zstd bag first (zstd -d)."
)]
struct Cli {
    /// The rosbag2 SQLite bag, `.db3`.
    bag: PathBuf,
    /// The MCAP recording to write.
    #[arg(short = 'o', long = "output", value_name = "file.mcap")]
    output: PathBuf,
    /// A directory of ROS 1 definitions, `<package>/<Name>.msg` or
    /// `<package>/msg/<Name>.msg`. Repeat for more; earlier ones win.
    #[arg(long = "msg-path", value_name = "dir")]
    msg_paths: Vec<PathBuf>,
    /// A topic to convert; repeat for more. Without one, every topic with a definition.
    #[arg(long = "topic", value_name = "name")]
    topics: Vec<String>,
    /// A `sensor_msgs/PointCloud2` topic whose XYZI clouds become the XYZIRC of
    /// Autoware 2.1.0; repeat for more.
    #[arg(long = "xyzirc", value_name = "name")]
    xyzirc: Vec<String>,
    /// Chunk compression.
    #[arg(short = 'c', long = "compression", value_name = "format", default_value = "zstd")]
    compression: McapCompression,
}

pub(crate) fn main(args: Vec<String>) -> ExitCode {
    let cli: Cli = match parse_cli(&args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let mut message_paths = cli.msg_paths;
    let installed = std::env::current_exe().ok().and_then(|exe| installed_message_path(&exe));
    message_paths.extend(installed);
    if message_paths.is_empty() {
        eprintln!(
            "hypha convert: no message path: give --msg-path, or run the hypha of an installed SDK"
        );
        return ExitCode::from(2);
    }
    let options = ConvertOptions {
        bag: cli.bag,
        output: cli.output,
        message_paths,
        topics: cli.topics,
        xyzirc: cli.xyzirc,
        compression: cli.compression,
    };
    match convert(&options) {
        Ok(report) => {
            for topic in &report.converted {
                print!(
                    "converted {} ({}): {} messages",
                    topic.topic, topic.data_type, topic.messages
                );
                match topic.highest_intensity {
                    Some(highest) if highest < 0.5 => println!(
                        "; XYZIRC, but every intensity rounded to 0: the highest recorded is \
                         {highest:e}, so the recording holds no intensity on the 0 to 255 scale"
                    ),
                    Some(highest) => println!("; XYZIRC, highest intensity {highest}"),
                    None => println!(),
                }
            }
            for topic in &report.skipped {
                println!("left out {} ({}): {}", topic.topic, topic.ros2_type, topic.reason);
            }
            println!("wrote {}", options.output.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("hypha convert: {error}");
            ExitCode::FAILURE
        }
    }
}
