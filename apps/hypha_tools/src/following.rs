//! The streams a tool follows, listed on a thread of the tool's own.
//!
//! Listing the streams of a domain takes tens of milliseconds, and a tool that reads a busy
//! topic cannot stop reading meanwhile: its queue is a few dozen samples. So the tool's loop only
//! subscribes to what a helper thread found, which is cheap, and the thread sleeps until the
//! graph event wakes it. The SDK creates no thread; this is the tool's (#209).

use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use hypha_node::sdk::{BusNode, FollowError, StreamFollower, StreamListings, TopicInfo};

/// How long the helper thread waits for the graph before it looks at its stop flag.
const STOP_CHECK: Duration = Duration::from_millis(200);

/// How long the helper thread waits after a failed read of the graph, so a broken event is
/// reported at a pace and not spun on.
const AFTER_FAILURE: Duration = Duration::from_secs(1);

/// Where the listings come from.
enum Source {
    /// Listed on the caller's thread when the graph changed, at once: for a reader that must
    /// follow every stream before it answers, and that a listing may block.
    Inline(Box<StreamListings>),
    /// Listed by a helper thread of this tool.
    Thread {
        listings: Receiver<Result<Vec<TopicInfo>, String>>,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    },
}

/// A follower and the listings that feed it.
pub struct Following {
    follower: StreamFollower,
    source: Source,
    /// What the first listing's streams refused, told by the first pass.
    refused: Option<FollowError>,
}

impl Following {
    /// Follows the streams `follower` matches, listing them on a helper thread: the first
    /// listing is made here, before the thread starts, so the streams that exist are followed
    /// when this returns.
    ///
    /// # Errors
    /// The graph event or the first listing failed. A stream that refuses its subscription is
    /// told by the first [`Following::pass`], and the others are followed.
    pub fn start(bus: &Arc<BusNode>, follower: StreamFollower) -> Result<Self, FollowError> {
        Self::open(bus, follower, true)
    }

    /// As [`Following::start`], with every listing made on the caller's thread: for a reader
    /// that must follow each stream before it answers, such as a recorder that takes part in a
    /// replay.
    ///
    /// # Errors
    /// As [`Following::start`].
    pub fn inline(bus: &Arc<BusNode>, follower: StreamFollower) -> Result<Self, FollowError> {
        Self::open(bus, follower, false)
    }

    fn open(
        bus: &Arc<BusNode>,
        mut follower: StreamFollower,
        threaded: bool,
    ) -> Result<Self, FollowError> {
        let mut listings = StreamListings::new(bus).map_err(FollowError::Listing)?;
        let first = listings.list(bus).map_err(FollowError::Listing)?;
        let refused = follower.apply(bus, &first).err();
        let source =
            if threaded { spawn(bus, listings) } else { Source::Inline(Box::new(listings)) };
        Ok(Self { follower, source, refused })
    }

    /// Follows what the listings found since the last call, and tells whether one came.
    ///
    /// # Errors
    /// A listing failed, or a stream refused its subscription; the others are followed.
    pub fn pass(&mut self, bus: &BusNode) -> Result<bool, FollowError> {
        let mut applied = false;
        let mut refusals = Vec::new();
        let mut failure = None;
        if let Some(FollowError::Refused(mut refused)) = self.refused.take() {
            refusals.append(&mut refused);
        }
        let mut take = |outcome: Result<Vec<TopicInfo>, String>, follower: &mut StreamFollower| {
            applied = true;
            match outcome {
                Ok(listing) => {
                    if let Err(FollowError::Refused(mut refused)) = follower.apply(bus, &listing) {
                        refusals.append(&mut refused);
                    }
                }
                Err(message) => failure = Some(FollowError::Listing(message)),
            }
        };
        match &mut self.source {
            Source::Inline(listings) => {
                if let Some(listing) = listings.list_if_changed(bus).transpose() {
                    take(listing, &mut self.follower);
                }
            }
            Source::Thread { listings, .. } => {
                while let Ok(outcome) = listings.try_recv() {
                    take(outcome, &mut self.follower);
                }
            }
        }
        match (failure, refusals.is_empty()) {
            (Some(error), _) => Err(error),
            (None, false) => Err(FollowError::Refused(refusals)),
            (None, true) => Ok(applied),
        }
    }
}

impl Deref for Following {
    type Target = StreamFollower;
    fn deref(&self) -> &StreamFollower {
        &self.follower
    }
}

impl DerefMut for Following {
    fn deref_mut(&mut self) -> &mut StreamFollower {
        &mut self.follower
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        if let Source::Thread { stop, handle, .. } = &mut self.source {
            stop.store(true, Ordering::Relaxed);
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
    }
}

/// Starts the helper thread that lists the streams when the graph changes.
fn spawn(bus: &Arc<BusNode>, mut listings: StreamListings) -> Source {
    let (sender, receiver) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = {
        let (bus, stop) = (Arc::clone(bus), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let outcome = listings.wait_for_listing(&bus, STOP_CHECK);
                let failed = outcome.is_err();
                let Some(outcome) = outcome.transpose() else { continue };
                if sender.send(outcome).is_err() {
                    return;
                }
                if failed {
                    std::thread::sleep(AFTER_FAILURE);
                }
            }
        })
    };
    Source::Thread { listings: receiver, stop, handle: Some(handle) }
}
