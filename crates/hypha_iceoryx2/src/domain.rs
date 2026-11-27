//! The domain: which whole system on this host a process belongs to.
//!
//! A domain gives each system on the host its own iceoryx2 resource prefix.

use std::ffi::OsString;
use std::sync::OnceLock;

use hypha_core::port::PortError;
use iceoryx2::config::Config;
use iceoryx2::prelude::{FileName, SemanticString as _};

/// The environment variable that selects the domain.
pub const DOMAIN_VARIABLE: &str = "HYPHA_DOMAIN";

/// The environment variable that makes this process part of a bounded deployment:
/// `1` turns it on, `0` or no variable leaves it off, and any other value is refused.
/// See [`bounded_memory`].
pub const BOUNDED_MEMORY_VARIABLE: &str = "HYPHA_BOUNDED_MEMORY";

/// The longest domain name.
pub const MAX_DOMAIN_LENGTH: usize = 32;

/// The resource prefix of `domain`: `iox2-<domain>-`. iceoryx2 lists a domain's
/// services by prefix, so no prefix may begin another; `-` is in no domain name.
fn domain_prefix(domain: &str) -> String {
    format!("iox2-{domain}-")
}

struct DomainConfig {
    domain: String,
    bounded_memory: bool,
    config: Config,
}

static DOMAIN_CONFIG: OnceLock<Result<DomainConfig, (PortError, String)>> = OnceLock::new();

/// True when `name` is `[a-z][a-z0-9_]*` and at most [`MAX_DOMAIN_LENGTH`] long.
#[must_use]
pub fn is_domain_name(name: &str) -> bool {
    hypha_core::port::is_identifier(name, MAX_DOMAIN_LENGTH)
}

/// Maximum received samples held by one subscriber port.
pub const MAX_BORROWED_SAMPLES: usize = 2;
/// Ports and nodes on an event service that names no limits of its own: the graph event.
const MAX_EVENT_PORTS: usize = 128;

/// The service limits of the domain, written over iceoryx2's defaults; they shape
/// only what the builders do not set.
///
/// iceoryx2 removes the resources of no dead node; hypha reclaims them when a node
/// starts (`transport.rs`). A dead node stays listed until a node of its name starts.
fn apply_hypha_defaults(config: &mut Config) {
    use iceoryx2::prelude::{AllocationStrategy, BackpressureStrategy};

    config.global.node.cleanup_dead_nodes_on_creation = false;
    config.global.node.cleanup_dead_nodes_on_destruction = false;
    config.global.service.cleanup_dead_nodes_on_open = false;

    let pubsub = &mut config.defaults.publish_subscribe;
    pubsub.max_subscribers = crate::qos::DEFAULT_MAX_SUBSCRIBERS;
    pubsub.max_publishers = crate::qos::DEFAULT_MAX_PUBLISHERS;
    pubsub.max_nodes = crate::qos::DEFAULT_MAX_PUBLISHERS + crate::qos::DEFAULT_MAX_SUBSCRIBERS;
    pubsub.subscriber_max_buffer_size = crate::qos::DEFAULT_MAX_DEPTH;
    pubsub.subscriber_max_borrowed_samples = MAX_BORROWED_SAMPLES;
    // A subscriber keeps a gone publisher's connection while it holds undelivered
    // samples: one slot per publisher, about 4 KB of heap each.
    pubsub.subscriber_expired_connection_buffer = crate::qos::DEFAULT_MAX_PUBLISHERS;
    pubsub.publisher_history_size = 1;
    pubsub.enable_safe_overflow = true;
    pubsub.backpressure_strategy = BackpressureStrategy::DiscardData;
    // Static by default: a growable publisher keeps 8 bytes × pool samples × 32 segments
    // of bookkeeping.
    pubsub.publisher_allocation_strategy = AllocationStrategy::Static;

    // Request-response (services): every hypha service sets these itself (`service/open.rs`);
    // the defaults keep any other port from a send that spins on its caller's thread.
    let services = &mut config.defaults.request_response;
    services.client_backpressure_strategy = BackpressureStrategy::DiscardData;
    services.server_backpressure_strategy = BackpressureStrategy::DiscardData;
    services.enable_safe_overflow_for_requests = false;
    services.enable_safe_overflow_for_responses = false;
    services.enable_fire_and_forget_requests = false;
    services.max_servers = 1;
    services.max_response_buffer_size = 1;
    services.max_borrowed_responses_per_pending_response = 1;
    services.max_loaned_requests = 1;
    services.server_max_loaned_responses_per_request = 1;

    let event = &mut config.defaults.event;
    event.max_notifiers = MAX_EVENT_PORTS;
    event.max_listeners = MAX_EVENT_PORTS;
    event.max_nodes = MAX_EVENT_PORTS;
    event.event_id_max_value = crate::notification::MAX_EVENT_ID;
}

/// Reads [`BOUNDED_MEMORY_VARIABLE`]: `1` is on, `0` and an unset variable are off.
fn parse_bounded_memory(variable: Option<OsString>) -> Result<bool, String> {
    let Some(raw) = variable else {
        return Ok(false);
    };
    match raw.to_str() {
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        _ => Err(format!(
            "{BOUNDED_MEMORY_VARIABLE}='{}' is not a setting: use 1 (bounded deployment) or 0",
            raw.display()
        )),
    }
}

/// The iceoryx2 configuration of `domain`, the empty name for the domain "none".
///
/// No host iceoryx2.toml is read, so two units of one domain cannot disagree about
/// the segment.
pub(crate) fn config_of(domain: &str) -> Result<Config, String> {
    let mut config = Config::default();
    if !domain.is_empty() {
        let prefix = domain_prefix(domain);
        config.global.prefix = FileName::new(prefix.as_bytes())
            .map_err(|cause| format!("the domain '{domain}' gives no valid prefix: {cause:?}"))?;
    }
    apply_hypha_defaults(&mut config);
    Ok(config)
}

fn build(variable: Option<OsString>) -> Result<DomainConfig, String> {
    let domain = match variable {
        None => String::new(),
        Some(raw) => {
            let Some(domain) = raw.to_str().map(str::to_string) else {
                return Err(format!(
                    "{DOMAIN_VARIABLE}='{}' is not UTF-8 text, so it is not a domain name",
                    raw.display()
                ));
            };
            if !is_domain_name(&domain) {
                return Err(format!(
                    "{DOMAIN_VARIABLE}='{domain}' is not a domain name: use [a-z][a-z0-9_], at most \
                     {MAX_DOMAIN_LENGTH} characters"
                ));
            }
            domain
        }
    };
    let config = config_of(&domain).map_err(|cause| format!("{DOMAIN_VARIABLE}: {cause}"))?;
    Ok(DomainConfig { domain, bounded_memory: false, config })
}

/// The domain of this process, built and checked at the first call.
fn domain_config() -> Result<&'static DomainConfig, PortError> {
    let built = DOMAIN_CONFIG.get_or_init(|| {
        let mut built = build(std::env::var_os(DOMAIN_VARIABLE))
            .map_err(|message| (PortError::InvalidDomain, format!("hypha: {message}")))?;
        built.bounded_memory = parse_bounded_memory(std::env::var_os(BOUNDED_MEMORY_VARIABLE))
            .map_err(|message| (PortError::InvalidArgument, format!("hypha: {message}")))?;
        crate::preflight::raise_open_file_limit();
        crate::preflight::check(&built.domain, &built.config)
            .map_err(|message| (PortError::InvalidState, message))?;
        Ok(built)
    });
    built.as_ref().map_err(|(error, message)| {
        log::error!(target: "hypha/transport", "{message}");
        *error
    })
}

/// The iceoryx2 configuration of this process's domain.
///
/// Fixed at the first call.
///
/// # Errors
/// Returns [`PortError::InvalidDomain`] when [`DOMAIN_VARIABLE`] holds an
/// invalid name or non-UTF-8 text; it never falls back to the domain "none".
/// [`PortError::InvalidState`] when the domain's shared files belong to another user.
pub fn config() -> Result<&'static Config, PortError> {
    domain_config().map(|domain_config| &domain_config.config)
}

/// The domain of this process, or an empty string for the domain "none".
///
/// # Errors
/// Returns [`PortError::InvalidDomain`] when [`DOMAIN_VARIABLE`] holds an
/// invalid name.
pub fn domain() -> Result<&'static str, PortError> {
    domain_config().map(|domain_config| domain_config.domain.as_str())
}

/// Whether this process runs in a bounded deployment, fixed at the first use of the
/// domain from [`BOUNDED_MEMORY_VARIABLE`].
///
/// In a bounded deployment the shared memory a domain can use is the sum of the
/// segments its publishers declare: a publisher without a payload limit is refused,
/// and so is one whose segment does not fit into the free space of `/dev/shm`.
///
/// # Errors
/// Returns [`PortError::InvalidArgument`] when [`BOUNDED_MEMORY_VARIABLE`] holds
/// anything but `1` or `0`, and the errors of [`config`].
pub fn bounded_memory() -> Result<bool, PortError> {
    domain_config().map(|domain_config| domain_config.bounded_memory)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_DOMAIN_LENGTH, config_of, domain_prefix, is_domain_name, parse_bounded_memory,
    };

    #[test]
    #[expect(clippy::expect_used, reason = "the domain none always has a configuration")]
    fn no_request_response_send_of_the_domain_retries_until_delivered() {
        use iceoryx2::prelude::BackpressureStrategy;

        let config = config_of("").expect("the domain none");
        let services = &config.defaults.request_response;
        assert_eq!(services.client_backpressure_strategy, BackpressureStrategy::DiscardData);
        assert_eq!(services.server_backpressure_strategy, BackpressureStrategy::DiscardData);
        assert!(!services.enable_fire_and_forget_requests);
        assert!(!services.enable_safe_overflow_for_requests);
        assert_eq!(services.max_servers, 1);
    }

    #[test]
    fn bounded_memory_is_on_for_1_off_for_0_or_unset_and_refuses_the_rest() {
        assert_eq!(parse_bounded_memory(None), Ok(false));
        assert_eq!(parse_bounded_memory(Some("0".into())), Ok(false));
        assert_eq!(parse_bounded_memory(Some("1".into())), Ok(true));
        for refused in ["", "true", "yes", "2", " 1"] {
            let message = parse_bounded_memory(Some(refused.into()));
            assert_eq!(
                message,
                Err(format!(
                    "HYPHA_BOUNDED_MEMORY='{refused}' is not a setting: use 1 (bounded deployment) or 0"
                ))
            );
        }
    }

    /// The resource prefix of the domain "none": iceoryx2's own default.
    const NO_DOMAIN_PREFIX: &str = "iox2_";

    #[test]
    fn no_domain_prefix_begins_another() {
        let longest = "a".repeat(MAX_DOMAIN_LENGTH);
        let domains =
            ["t", "t_1", "t_3683_8", "test", "test_0437ec0fddc15d015b292d7c7a4", &longest];
        let mut prefixes = vec![NO_DOMAIN_PREFIX.to_owned()];
        for domain in domains {
            assert!(is_domain_name(domain), "{domain}");
            prefixes.push(domain_prefix(domain));
        }
        for first in &prefixes {
            for second in &prefixes {
                assert!(
                    first == second || !second.starts_with(first.as_str()),
                    "{first} begins {second}"
                );
            }
        }
    }
}
