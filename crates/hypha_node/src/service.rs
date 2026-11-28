//! Services in plain nodes and `ApplicationNode`s: typed request and response between
//! nodes, from a ROS 1 `.srv` (`docs/design/native-services.md`).
//!
//! Every rule of a call lives here once: how a declaration resolves against the ports
//! tables, when a call is refused locally, which outcome ends it, that a blocking call is
//! refused on a dispatch thread, and that an unanswered request is refused. The C ABI and
//! the SDKs only carry bytes and callbacks across.

mod call_log;
mod client;
mod cycle;
mod listing;
mod outcome;
mod server;
mod spec;

pub use call_log::{CALL_LOG_POLICY, SERVICE_CALL_DEFINITION, call_log_topic};
pub use client::{
    CallDone, ClientStatus, HyphaClient, client_call, client_status, client_wait_for_server,
    service_client_create, service_client_destroy,
};
pub(crate) use client::{ClientCore, Finished};
pub use cycle::{CycleServices, DEFAULT_CALLS_OUTSTANDING, DEFAULT_REQUESTS_PER_STEP, StepRequest};
pub use listing::{ListedService, ServiceDefinitions, list_services, service_definitions};
pub use outcome::{
    CallOutcome, CallResult, CallStatus, HYPHA_CALL_ANSWERED, HYPHA_CALL_BUSY,
    HYPHA_CALL_NO_SERVER, HYPHA_CALL_NOT_SENT, HYPHA_CALL_REFUSED, HYPHA_CALL_SENT,
    HYPHA_CALL_SERVER_LOST, HYPHA_CALL_TIMEOUT, HYPHA_CALL_TOO_LARGE,
};
pub(crate) use server::ServerCore;
pub use server::{
    Answer, HyphaRequest, HyphaServer, RequestView, ServeHandler, request_answer, request_defer,
    server_unanswered, service_serve, service_server_destroy,
};
pub use spec::ServiceDecl;
