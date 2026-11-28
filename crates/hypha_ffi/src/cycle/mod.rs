//! The C ABI wrappers of `hypha_node::cycle`.

mod driver;
mod handle;
mod image;
mod input_supervision;
mod inspect;
mod registration;
mod source;
mod tick;
mod transforms;

pub use driver::{hypha_cycle_expect_participant, hypha_cycle_set_clock_driver};
pub use handle::HyphaCycle;
pub(crate) use handle::{CycleTarget, OwnedHandle, snapshots_of};
pub use image::{
    hypha_cycle_emission_commit, hypha_cycle_emission_reserve, hypha_cycle_image_alive,
    hypha_cycle_image_binding, hypha_cycle_image_clear, hypha_cycle_image_held,
    hypha_cycle_image_reject_sample, hypha_cycle_image_sample, hypha_cycle_image_set_ignored,
};
pub use input_supervision::hypha_cycle_bind_input_supervision;
pub use inspect::{hypha_cycle_binding, hypha_cycle_health};
pub use registration::{
    hypha_cycle_bind_input, hypha_cycle_bind_output, hypha_cycle_commit, hypha_cycle_create,
    hypha_cycle_destroy, hypha_cycle_set_class_based, hypha_cycle_set_host_clock,
    hypha_cycle_set_trigger,
};
pub use source::{hypha_input_source_create, hypha_input_source_destroy};
pub use tick::cycle_timer_tick;
pub use transforms::{hypha_cycle_bind_transforms, hypha_cycle_image_lookup_transform};
