//! A follower fed by its listings on the test's own thread, as a tool without a helper thread
//! would run it.
#![allow(clippy::expect_used, dead_code)]

use hypha_node::sdk::{BusNode, FollowError, StreamFollower, StreamListings, StreamSelector};

/// A follower and the listings that feed it.
pub(crate) struct Following {
    pub(crate) follower: StreamFollower,
    listings: StreamListings,
}

impl Following {
    pub(crate) fn new(
        bus: &BusNode,
        keys: Vec<String>,
        selector: StreamSelector,
        depth: usize,
    ) -> Self {
        Self {
            follower: StreamFollower::new(keys, selector, depth),
            listings: StreamListings::new(bus).expect("listings"),
        }
    }

    /// Lists now and follows what matches.
    pub(crate) fn refresh(&mut self, bus: &BusNode) -> Result<usize, FollowError> {
        let listing = self.listings.list(bus).map_err(FollowError::Listing)?;
        self.follower.apply(bus, &listing)
    }

    /// Lists and follows when the graph changed, at once.
    pub(crate) fn settle(&mut self, bus: &BusNode) -> Result<usize, FollowError> {
        match self.listings.list_if_changed(bus).map_err(FollowError::Listing)? {
            Some(listing) => self.follower.apply(bus, &listing),
            None => Ok(0),
        }
    }
}

impl std::ops::Deref for Following {
    type Target = StreamFollower;
    fn deref(&self) -> &StreamFollower {
        &self.follower
    }
}

impl std::ops::DerefMut for Following {
    fn deref_mut(&mut self) -> &mut StreamFollower {
        &mut self.follower
    }
}
