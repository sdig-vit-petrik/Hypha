//! Services for tools and tests: the services of the domain, the definitions their servers
//! registered, and a blocking client (`docs/design/native-services.md` §8.8).

use std::sync::mpsc;
use std::time::Duration;

use crate::sdk::BusNode;
use crate::sdk::ffi;
use crate::sdk::status::named_error;

pub use crate::safe::{CallOutcome, CallResult, ListedService, ServiceDefinitions, call_log_topic};
pub use hypha_core::port::ServicePolicy;

/// A blocking client of one service stream: each call waits on the calling thread for its
/// one outcome. Made by [`BusNode::service_client`].
pub struct ServiceClient {
    client: ffi::HyphaClient,
    max_request_bytes: Option<usize>,
}

impl BusNode {
    /// Every service stream of the domain: name, type, MD5, policy, server and clients.
    ///
    /// # Errors
    /// A message when the registry cannot be read.
    pub fn list_services(&self) -> Result<Vec<ListedService>, String> {
        ffi::list_services(&self.node)
            .map_err(|error| named_error("cannot list the services", error))
    }

    /// The request and response definitions the server of `service`'s stream of `md5`
    /// registered; `Ok(None)` when it registered none.
    ///
    /// # Errors
    /// A message for a malformed MD5 or a store that cannot be read.
    pub fn service_definitions(
        &self,
        service: &str,
        md5: &str,
    ) -> Result<Option<ServiceDefinitions>, String> {
        ffi::service_definitions(&self.node, service, md5).map_err(|error| {
            named_error(&format!("cannot read the definitions of '{service}'"), error)
        })
    }

    /// A blocking client of `service` (a resolved name) of `type_name` and `md5`, under
    /// `policy` (`None`: `defaultService`). `timeout` overrides the policy's. A client of a
    /// tool node takes an observer slot, never a declared client's.
    ///
    /// # Errors
    /// A message with the library's reason when the client is refused.
    pub fn service_client(
        &self,
        service: &str,
        type_name: &str,
        md5: &str,
        policy: Option<ServicePolicy>,
        timeout: Option<Duration>,
    ) -> Result<ServiceClient, String> {
        let max_request_bytes = policy.as_ref().map(|policy| policy.max_request_bytes);
        let decl = ffi::ServiceDecl {
            name: service,
            literal: true,
            type_name,
            md5,
            request_definition: "",
            response_definition: "",
            policy,
            timeout,
        };
        ffi::service_client_create(&self.node, &decl, true)
            .map(|client| ServiceClient { client, max_request_bytes })
            .map_err(|error| {
                let reason = ffi::last_error();
                let what = format!("cannot call '{service}'");
                if reason.is_empty() {
                    named_error(&what, error)
                } else {
                    format!("{what}: {reason}")
                }
            })
    }
}

impl ServiceClient {
    /// Call with the encoded `request` and wait for the outcome: at most the client's
    /// timeout, after a server appears or not. A call refused before anything was sent
    /// (calls in flight at the limit, a request above the bound) ends `NotSent`.
    ///
    /// # Errors
    /// A message when the call is refused on a thread that runs a dispatch.
    pub fn call_and_wait(&self, request: &[u8]) -> Result<CallResult, String> {
        let (sender, receiver) = mpsc::channel();
        let done = move |result: &CallResult| {
            let _ = sender.send(result.clone());
            crate::HyphaCallbackResult::continue_run()
        };
        let status = ffi::client_call(&self.client, request, Box::new(done))
            .map_err(|error| named_error("callAndWait", error))?;
        if let Ok(result) = receiver.try_recv() {
            return Ok(result);
        }
        let why = match status {
            crate::safe::CallStatus::TooLarge => {
                let bound = self
                    .max_request_bytes
                    .map_or_else(String::new, |bytes| format!(" of {bytes} B"));
                format!("the request of {} B is above the bound{bound}", request.len())
            }
            crate::safe::CallStatus::Busy | crate::safe::CallStatus::Sent => {
                "the client has its limit of calls in flight".to_owned()
            }
        };
        let now = ffi::clock_now_nanos();
        Ok(CallResult { outcome: CallOutcome::NotSent(why), sent_nanos: now, done_nanos: now })
    }

    /// Wait up to `timeout` for a live server; true when one is there.
    ///
    /// # Errors
    /// A message when the wait is refused on a thread that runs a dispatch.
    pub fn wait_for_server(&self, timeout: Duration) -> Result<bool, String> {
        ffi::client_wait_for_server(&self.client, timeout)
            .map_err(|error| named_error("waitForServer", error))
    }
}
