//! `hypha topic pool`: the shared memory a publisher's pool reserves for a policy and a
//! message size, from the one rule of the transport.

use clap::Args;
use hypha_node::sdk::{DEFAULT_LOANED_SAMPLES, PoolSize, UNDECLARED_SUBSCRIBER_SLOTS, pool_size};
use std::process::ExitCode;

/// The limits of a policy and the size of a message. A limit not given is that of hypha's
/// `default` policy.
#[derive(Debug, Args)]
pub(crate) struct PoolArgs {
    /// The largest message of the publisher, serialized, in bytes: what the port's
    /// `maxPayloadDynamic` says.
    #[arg(long, value_name = "bytes")]
    payload: usize,
    /// Publishers the policy admits (the pool is the pool of each).
    #[arg(long, value_name = "n", default_value_t = 8)]
    publishers: usize,
    /// Subscribers the policy admits, the observer slots not counted.
    #[arg(long, value_name = "n", default_value_t = 16)]
    subscribers: usize,
    /// The deepest queue a subscriber may ask for.
    #[arg(long, value_name = "n", default_value_t = 32)]
    depth: usize,
    /// Loans the publisher holds.
    #[arg(long, value_name = "n", default_value_t = DEFAULT_LOANED_SAMPLES)]
    loans: usize,
}

/// The text of the answer: the formula's terms, then the pool of one publisher and of the
/// topic when every publisher of the policy runs.
fn report(args: &PoolArgs, pool: &PoolSize) -> String {
    let total = pool.bytes.saturating_mul(u64::try_from(args.publishers).unwrap_or(u64::MAX));
    format!(
        "policy: publishers {}, subscribers {} (+{UNDECLARED_SUBSCRIBER_SLOTS} observer slots), depth {}; \
         loans {}; messages of {} bytes\n\
         pool of one publisher: {} samples of {} bytes (the message and 104 bytes of headers) = {}\n\
         the topic when all {} publishers run: {}\n",
        args.publishers,
        args.subscribers,
        args.depth,
        args.loans,
        args.payload,
        pool.samples,
        pool.sample_bytes,
        size_text(pool.bytes),
        args.publishers,
        size_text(total)
    )
}

/// `bytes` in bytes and in the larger unit that reads best.
fn size_text(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    if bytes >= GIB {
        format!(
            "{bytes} bytes ({:.2} GiB)",
            hypha_tools::approx_f64(bytes) / hypha_tools::approx_f64(GIB)
        )
    } else if bytes >= MIB {
        format!(
            "{bytes} bytes ({:.1} MiB)",
            hypha_tools::approx_f64(bytes) / hypha_tools::approx_f64(MIB)
        )
    } else {
        format!("{bytes} bytes")
    }
}

pub(crate) fn run(args: &PoolArgs) -> ExitCode {
    match pool_size(args.publishers, args.subscribers, args.depth, args.loans, args.payload) {
        Ok(pool) => {
            print!("{}", report(args, &pool));
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("hypha topic pool: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a failed setup is a test failure")]
mod tests {
    use super::*;

    fn args(publishers: usize, subscribers: usize, depth: usize, payload: usize) -> PoolArgs {
        PoolArgs { payload, publishers, subscribers, depth, loans: 2 }
    }

    #[test]
    fn the_report_names_the_pool_of_a_camera() {
        let camera = args(1, 2, 2, 5_000_048);
        let pool = pool_size(1, 2, 2, 2, 5_000_048).expect("a camera policy is valid");
        let text = report(&camera, &pool);
        assert!(text.contains("23 samples of 5000152 bytes"), "{text}");
        assert!(text.contains("115003496 bytes (109.7 MiB)"), "{text}");
    }

    #[test]
    fn a_limit_outside_a_policy_is_an_error() {
        assert!(pool_size(0, 2, 2, 2, 100).is_err());
        assert!(pool_size(1, 2, 5000, 2, 100).is_err());
    }
}
