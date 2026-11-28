//! Shared input history and cycle execution across the C ABI.

pub mod driver;
pub mod image;
pub mod input_supervision;
pub mod inspect;
pub mod introspection;
pub mod registration;
pub mod source;
pub(crate) use source::{DrainClock, DrainStep, drain_at_most};
pub mod state;
pub mod tick;
pub mod transforms;
pub mod types;

pub(crate) use driver::DriverSetup;
pub use driver::{cycle_expect_participant, cycle_set_clock_driver};
pub use image::{
    EmissionView, HyphaCycleEmission, HyphaCycleImage, ImageView, emission_call, emission_commit,
    emission_next_result, emission_requests, emission_reserve, emission_unread_results,
    image_alive, image_binding, image_held, image_reject_sample, image_sample, image_set_ignored,
    with_service_emission,
};
pub use input_supervision::{
    HyphaInputSupervisionRequest, HyphaInputSupervisionResult, cycle_bind_input_supervision,
};
pub use inspect::{cycle_binding, cycle_health};
pub use registration::{
    cycle_bind_input, cycle_bind_output, cycle_commit, cycle_create, cycle_destroy,
    cycle_set_class_based, cycle_set_host_clock, cycle_set_trigger,
};
pub use source::{HyphaInputSource, input_source_create, input_source_destroy};
pub use transforms::{cycle_bind_transforms, image_lookup_transform};
pub use types::{
    HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, HYPHA_CYCLE_VERDICT_STALE, HYPHA_CYCLE_VERDICT_USABLE,
    HYPHA_DEFAULT_ARRIVALS_PER_STEP, HYPHA_FAULT_STAGNANT, HYPHA_FAULT_STALE,
    HyphaClockDriverOptions, HyphaCycleBindingHealth, HyphaCycleBindingOptions,
    HyphaCycleBindingReport, HyphaCycleBindingView, HyphaCycleHealth, HyphaCycleHeldReport,
    HyphaCycleInputMode, HyphaCycleOutputFailure, HyphaCycleSampleView, HyphaCycleStep,
    HyphaCycleTime, HyphaSupervisionLimits, HyphaSupervisionReport,
};

pub(crate) use hypha_cycle::TIME_BASE_BOUND_PERIODS;
pub(crate) use source::CycleInputSource;
pub(crate) use source::{check_input_publishers, drain_inputs};
pub(crate) use state::{Cycle, InputCounts};
pub use state::{CycleId, CycleSnapshots, CycleStepHandler};
pub use tick::cycle_tick;
pub(crate) use tick::{cycle_timer_tick, report_time_base};
