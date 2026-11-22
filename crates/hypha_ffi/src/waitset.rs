//! The C ABI wrappers of `hypha_node::waitset`, and its C types.

pub use crate::abi_constants::{
    HYPHA_ACTION_CONTINUE, HYPHA_ACTION_FAIL, HYPHA_ACTION_STOP, HYPHA_ATTACH_SUBSCRIPTION,
    HYPHA_ATTACH_TIMER, HYPHA_RUN_REASON_DEADLINE, HYPHA_RUN_REASON_RUNTIME_FAILED,
    HYPHA_RUN_REASON_SETUP_FAILED, HYPHA_RUN_REASON_STOPPED,
};
pub use hypha_node::waitset::{
    HyphaAttachment, HyphaAttachmentId, HyphaCallbackResult, HyphaContextRelease, HyphaDelivery,
    HyphaIntervalCallback, HyphaSubscriptionCallback, HyphaTimerState, HyphaWaitSet,
    HyphaWaitSetRunReport,
};

pub(crate) mod access;
pub(crate) mod configuration;
mod registrations;
mod run;
mod timers;
pub use configuration::hypha_node_default_waitset;
pub use registrations::{hypha_waitset_attach, hypha_waitset_detach};
pub use run::{hypha_waitset_request_failure, hypha_waitset_request_stop, hypha_waitset_run};
pub use timers::{
    hypha_waitset_timer_cancel, hypha_waitset_timer_reset, hypha_waitset_timer_state,
};
