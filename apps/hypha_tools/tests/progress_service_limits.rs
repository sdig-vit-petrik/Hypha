//! `/clock_progress` has one service limit for the whole domain.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[allow(dead_code, reason = "each test binary uses part of the shared helpers")]
mod common;

use hypha_node::sdk::BusNode;
use hypha_tools::record_sync::SyncParticipant;
use hypha_tools::replay_bus::progress_subscriber;

/// Creates a node on simulated time, as a node of a replay is. The default is read when a
/// node is created, so the tools' own bus nodes are created before this runs.
fn sim_time_node(name: &str) -> BusNode {
    hypha_testkit::set_env("HYPHA_USE_SIM_TIME", "1");
    BusNode::new(name).expect("a simulated-time node after the tools' endpoints")
}

#[test]
fn a_player_opens_its_endpoints_first_and_a_node_still_starts() {
    let player = BusNode::new("hypha_play").expect("the player's bus node");
    let _progress = progress_subscriber(&player).expect("the player's progress input");

    let _node = sim_time_node("posmaster_track_node");
}

#[test]
fn a_recorder_opens_its_endpoints_first_and_a_node_still_starts() {
    let recorder = BusNode::new("hypha_record").expect("the recorder's bus node");
    let _sync = SyncParticipant::open(&recorder, Vec::new()).expect("the recorder's endpoints");

    let _node = sim_time_node("posmaster_track_node");
}

#[test]
fn a_node_that_starts_first_accepts_the_players_and_recorders_endpoints() {
    let player = BusNode::new("hypha_play").expect("the player's bus node");
    let recorder = BusNode::new("hypha_record").expect("the recorder's bus node");
    let _node = sim_time_node("posmaster_track_node");

    let _progress = progress_subscriber(&player).expect("the player's progress input");
    let _sync = SyncParticipant::open(&recorder, Vec::new()).expect("the recorder's endpoints");
}
