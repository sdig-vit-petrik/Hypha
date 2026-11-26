//! The chunk loop: closes chunks at their boundaries, publishes each message, and ends a pass.

use super::{CLOCK_TOPIC, Flow, Playback, Session};
use crate::player::PlaybackItem;
use std::time::Instant;

impl Session<'_> {
    /// Closes one chunk: waits for the schedule to reach its end, then sends `/clock` for
    /// it and, in the lockstep, waits for the participants.
    pub(super) fn close_chunk(&mut self, boundary: u64) -> Result<Flow, String> {
        // Without `/clock` there is no barrier, and a chunk closes as soon as its data is out.
        if self.clock.is_some() {
            if self.pace_to(boundary)? == Flow::Halt {
                return Ok(Flow::Halt);
            }
            if self.barrier(boundary)? == Flow::Halt {
                return Ok(Flow::Halt);
            }
            self.account_data(boundary);
            self.in_chunk.clear();
            self.limits.clear();
        }
        if self.playback == Playback::Stepping {
            // The step's chunk is closed: the schedule is at its end, and the replay waits.
            self.playback = Playback::Paused;
            self.schedule.reanchor(boundary, Instant::now());
        }
        Ok(Flow::Go)
    }

    /// Publishes one message. A refused publish fails the replay under `/clock`, where the
    /// chunks promise that every message went out; without `/clock` it is only reported.
    pub(super) fn publish_item(&mut self, item: &PlaybackItem<'_>) -> Result<(), String> {
        self.last_log_time = item.log_time_nanos;
        if self.last_topic != item.topic {
            self.last_topic.clear();
            self.last_topic.push_str(item.topic);
        }
        let Some(publisher) = self.publishers.get_mut(item.topic) else {
            // Under `/clock` the recording's own clock is replaced by the replay's clock,
            // so it has no publisher on purpose.
            let recorded_clock_is_replaced = self.config.publish_clock && item.topic == CLOCK_TOPIC;
            if recorded_clock_is_replaced {
                return Ok(());
            }
            return Err(format!("no publisher for '{}': the replay did not start it", item.topic));
        };
        // The frame carries the message's log time as its framework stamp.
        if publisher.publish_at_framework_nanos(item.bytes, item.log_time_nanos) {
            count_one(&mut self.counters, item.topic);
            Ok(())
        } else if self.clock.is_some() {
            Err(format!("cannot publish '{}': a message of the replay would be lost", item.topic))
        } else {
            eprintln!("hypha play: failed to publish '{}'", item.topic);
            Ok(())
        }
    }

    /// Closes every boundary before `log_time`. Each is taken in turn, so a long gap
    /// allocates nothing.
    fn close_boundaries_before(&mut self, log_time: u64) -> Result<Flow, String> {
        loop {
            let next = match self.plan.as_mut() {
                Some(plan) => plan.next_closed_before(log_time)?,
                None => None,
            };
            let Some(boundary) = next else {
                return Ok(Flow::Go);
            };
            if self.close_chunk(boundary)? == Flow::Halt {
                return Ok(Flow::Halt);
            }
        }
    }

    /// Handles one message of the pass: closes the chunks that end before it, paces it,
    /// and publishes it.
    pub(super) fn step_item(&mut self, item: &PlaybackItem<'_>) -> Result<Flow, String> {
        self.current_msg = self.current_msg.saturating_add(1);
        if self.plan.is_none() && self.start_pass(item.log_time_nanos)? == Flow::Halt {
            return Ok(Flow::Halt);
        }
        if self.close_boundaries_before(item.log_time_nanos)? == Flow::Halt {
            return Ok(Flow::Halt);
        }
        if self.split_full_chunk(item)? == Flow::Halt {
            return Ok(Flow::Halt);
        }
        if self.pace_to(item.log_time_nanos)? == Flow::Halt {
            return Ok(Flow::Halt);
        }
        self.publish_item(item)?;
        if self.clock.is_some() {
            count_one_in_chunk(&mut self.in_chunk, item.topic);
        }
        self.render_if_due();
        Ok(Flow::Go)
    }

    /// Closes the chunk before `item` when its topic already fills the smallest subscriber
    /// queue, so that no subscriber loses samples within one chunk. The chunk closes one
    /// nanosecond before the item, so it holds every message stamped at or before its close,
    /// as a grid chunk does. Without `/clock`, or without the lockstep that would wait for
    /// it, nothing is split.
    fn split_full_chunk(&mut self, item: &PlaybackItem<'_>) -> Result<Flow, String> {
        if !self.in_lockstep() {
            return Ok(Flow::Go);
        }
        let sent = self.in_chunk.get(item.topic).copied().unwrap_or(0);
        if sent == 0 {
            return Ok(Flow::Go);
        }
        let Some(publisher) = self.publishers.get(item.topic) else {
            return Ok(Flow::Go);
        };
        let limit = if let Some(limit) = self.limits.get(item.topic) {
            *limit
        } else {
            let limit = publisher.min_subscriber_buffer()?;
            self.limits.insert(item.topic.to_owned(), limit);
            limit
        };
        let Some(limit) = limit else {
            return Ok(Flow::Go);
        };
        if sent < limit {
            return Ok(Flow::Go);
        }
        let Some(boundary) =
            self.plan.as_mut().and_then(|plan| plan.shorten_before(item.log_time_nanos))
        else {
            return Err(format!(
                "{} messages of '{}' at one instant exceed the smallest subscriber queue {limit}",
                sent.saturating_add(1),
                item.topic
            ));
        };
        if !self.shortened.contains(item.topic) && self.shortened.insert(item.topic.to_owned()) {
            println!(
                "hypha play: chunks shortened on '{}': its smallest subscriber queue holds {limit} samples",
                item.topic
            );
        }
        self.close_chunk(boundary)
    }

    /// Called for each message of the pass. Returns false to stop delivering.
    pub(super) fn on_item(&mut self, item: &PlaybackItem<'_>) -> bool {
        match self.step_item(item) {
            Ok(Flow::Go) => true,
            Ok(Flow::Halt) => {
                self.halted_at = Some(item.log_time_nanos);
                false
            }
            Err(error) => {
                self.failure = Some(error);
                false
            }
        }
    }

    /// Ends a pass. The last chunks are closed like any other, so a rewind or a loop starts
    /// only once the participants have the tail of the pass. A restart stops before its
    /// next message; the chunk that it leaves open is closed one nanosecond before that
    /// message, the first one not yet sent. A quit closes nothing.
    pub(super) fn finish_pass(&mut self) {
        if self.failure.is_some() || self.stopping() || self.plan.is_none() {
            return;
        }
        let target = match self.halted_at {
            // The clock never goes back within a pass: a restart at its first instant sends
            // the start's clock again.
            Some(log_time) => log_time.saturating_sub(1).max(self.last_boundary),
            None => match self.close_tail() {
                Ok(None) => return,
                Ok(Some(end)) => end,
                Err(error) => {
                    self.failure = Some(error);
                    return;
                }
            },
        };
        if self.clock.is_none() || self.stopping() {
            return;
        }
        match self.barrier(target) {
            Ok(Flow::Go) => self.account_data(target),
            Ok(Flow::Halt) => {}
            Err(error) => self.failure = Some(error),
        }
    }

    /// Closes the chunks after the last message of a pass: a boundary at that message, as a
    /// message one nanosecond later would close it, then the next boundary, which ends the
    /// pass, so that a step after the last message runs. The final `/clock` is capped at
    /// the window's exclusive end, which may be off the grid. Returns the end when a restart
    /// halted the closing, for a barrier that still closes it.
    fn close_tail(&mut self) -> Result<Option<u64>, String> {
        let after_last = self.last_log_time.saturating_add(1);
        let flow = self.close_boundaries_before(after_last)?;
        let Some(end) =
            self.plan.as_ref().map(|plan| plan.end_boundary(self.config.utc_range.end()))
        else {
            return Ok(None);
        };
        if flow == Flow::Go && self.close_chunk(end)? == Flow::Go {
            return Ok(None);
        }
        Ok(Some(end))
    }

    pub(super) fn counters_in_order(&self) -> Vec<(String, u64)> {
        self.topic_order
            .iter()
            .map(|topic| (topic.clone(), *self.counters.get(topic).unwrap_or(&0)))
            .collect()
    }
}
/// Adds one to `topic`'s count, allocating its key only the first time.
fn count_one(counts: &mut std::collections::HashMap<String, u64>, topic: &str) {
    if let Some(count) = counts.get_mut(topic) {
        *count = count.saturating_add(1);
    } else {
        counts.insert(topic.to_owned(), 1);
    }
}

/// [`count_one`] for the per-chunk counts.
fn count_one_in_chunk(counts: &mut std::collections::HashMap<String, usize>, topic: &str) {
    if let Some(count) = counts.get_mut(topic) {
        *count = count.saturating_add(1);
    } else {
        counts.insert(topic.to_owned(), 1);
    }
}
