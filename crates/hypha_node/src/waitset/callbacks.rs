//! `WaitSet` callbacks ownership and C ABI implementation.

use crate::node_progress::ClockRead;

use super::{
    DataCallback, ExecutionHealth, FrameIdentity, HyphaCallbackResult, HyphaDelivery,
    HyphaSampleIdentity, HyphaWaitSet, IntervalHandler, SampleDelivery, host_now_nanos,
};

/// Deliver one validated sample. `received_nanos` is the host time validation
/// read for it, or zero when validation read no clock. A C callback may keep the sample
/// within `budget` (`hypha_delivery_retain`); otherwise it is released on return.
pub(super) fn invoke_data(
    health: &ExecutionHealth,
    callback: &mut DataCallback,
    sample: hypha_iceoryx2::Iox2Sample,
    received_nanos: u64,
    set: &HyphaWaitSet,
    budget: &std::sync::Arc<crate::subscriber::loan_budget::LoanBudget>,
) -> HyphaCallbackResult {
    let answer = match callback {
        DataCallback::Foreign(None) => return HyphaCallbackResult::continue_run(),
        DataCallback::Foreign(Some(foreign)) => {
            // The C view copies the addresses: the payload and the topic stay where they
            // are when the holder hands the sample to the callback.
            let mut delivery = HyphaDelivery::from_view(&sample_delivery(&sample, received_nanos));
            let holder =
                crate::sample::HyphaDeliveryHolder::new(sample, std::sync::Arc::clone(budget));
            delivery.holder = &raw const holder;
            foreign.deliver(&delivery)
        }
        DataCallback::Native(handler) => {
            handler.deliver(&sample_delivery(&sample, received_nanos), set).as_result()
        }
    };
    health.recorded(answer)
}

/// Set simulated time from one `/clock` sample, and return what the sample carried.
pub(crate) fn apply_sim_clock_sample(
    sample: &hypha_iceoryx2::Iox2Sample,
) -> Result<ClockRead, crate::status::Error> {
    let read = read_sim_clock_sample(sample)?;
    apply_sim_clock_read(read)?;
    Ok(read)
}

/// What one `/clock` sample carries; `CorruptPayload` for one that is no clock.
pub(super) fn read_sim_clock_sample(
    sample: &hypha_iceoryx2::Iox2Sample,
) -> Result<ClockRead, crate::status::Error> {
    let nanos = hypha_wire::ros_time::RosTime::decode_clock(sample.payload())
        .map(hypha_wire::ros_time::RosTime::as_nanos)
        .ok_or(crate::status::Error::CorruptPayload)?;
    Ok(ClockRead { nanos, sent_nanos: sample.header().metadata.transport_sent_nanos })
}

/// Set simulated time to the value of `read`.
pub(super) fn apply_sim_clock_read(read: ClockRead) -> Result<(), crate::status::Error> {
    let signed = i64::try_from(read.nanos).map_err(|_| crate::status::Error::CorruptPayload)?;
    crate::clock::clock_set_sim_time(signed)
}

/// Call a C timer callback for one due period; nothing for a released one.
pub(super) fn invoke_interval(
    health: &ExecutionHealth,
    callback: &mut Option<Box<dyn IntervalHandler>>,
    missed: u64,
) -> HyphaCallbackResult {
    let Some(callback) = callback.as_mut() else {
        return HyphaCallbackResult::continue_run();
    };
    health.recorded(callback.tick(missed))
}

/// Step `cycle` of `set` once, and count the answer in `health`.
pub(super) fn invoke_cycle(
    health: &ExecutionHealth,
    cycle: &crate::cycle::Cycle,
    set: &HyphaWaitSet,
    missed: u64,
) -> HyphaCallbackResult {
    health.recorded(crate::cycle::cycle_timer_tick(cycle, set, missed))
}

/// The delivery of one sample, which the dispatcher has validated.
fn sample_delivery(sample: &hypha_iceoryx2::Iox2Sample, received_nanos: u64) -> SampleDelivery<'_> {
    let header = sample.header().metadata;
    // Validation read the clock for any frame with a send time, so only a frame stamped
    // zero needs a read here.
    let now = if received_nanos == 0 { host_now_nanos() } else { received_nanos };
    let age = now.saturating_sub(header.transport_sent_nanos);
    SampleDelivery {
        payload: sample.payload(),
        identity: FrameIdentity {
            topic: sample.topic(),
            transport_sent_nanos: header.transport_sent_nanos,
            framework_sent_nanos: header.framework_sent_nanos,
            schema_hash: header.schema_hash,
        },
        transport_age_nanos: age,
    }
}

impl HyphaDelivery {
    /// The C view of `delivery`, which borrows its payload and topic.
    fn from_view(delivery: &SampleDelivery<'_>) -> Self {
        Self {
            payload: delivery.payload.as_ptr(),
            payload_len: delivery.payload.len(),
            transport_age_nanos: delivery.transport_age_nanos,
            identity: HyphaSampleIdentity::from_view(&delivery.identity),
            holder: std::ptr::null(),
        }
    }
}
