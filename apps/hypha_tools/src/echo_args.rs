//! Command line of `hypha topic echo`.

use crate::stream::StreamFlags;
use clap::Args;

/// What `hypha topic echo` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct EchoArgs {
    /// Topic to print.
    pub topic: String,
    /// Stop after this many messages; 0 means no limit.
    #[arg(long = "count", value_name = "n", default_value_t = 0)]
    pub max_count: usize,
    /// Print one CSV row for each message.
    #[arg(short = 'p')]
    pub plot: bool,
    /// Which streams of the topic to print.
    #[command(flatten)]
    pub streams: StreamFlags,
}
