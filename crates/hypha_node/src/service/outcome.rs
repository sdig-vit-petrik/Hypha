//! What became of a call: exactly one outcome per call that was sent (FSR-42).
//!
//! The rules of an outcome (its text, whether the request may have run, whether a blind
//! retry is safe) are written here once; the C ABI and the SDKs read them.

use std::fmt;

/// `call()` sent the request, or holds it until a server appears.
pub const HYPHA_CALL_SENT: u32 = 0;
/// `call()` refused: the client has its limit of calls in flight.
pub const HYPHA_CALL_BUSY: u32 = 1;
/// `call()` refused: the request is above the service's bound.
pub const HYPHA_CALL_TOO_LARGE: u32 = 2;

/// The server answered.
pub const HYPHA_CALL_ANSWERED: u32 = 0;
/// The server refused, or left the request unanswered.
pub const HYPHA_CALL_REFUSED: u32 = 1;
/// No live server of the stream until the timeout.
pub const HYPHA_CALL_NO_SERVER: u32 = 2;
/// Nothing was sent.
pub const HYPHA_CALL_NOT_SENT: u32 = 3;
/// The server crashed or left, failed while it handled the call, or its answer did not
/// verify.
pub const HYPHA_CALL_SERVER_LOST: u32 = 4;
/// No answer within the timeout.
pub const HYPHA_CALL_TIMEOUT: u32 = 5;

/// What `call()` did with a request: sent it (`Sent`), or refused it locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// Sent, or held until a server appears; exactly one outcome follows.
    Sent,
    /// The client has its limit of calls in flight, expired calls the server still holds
    /// included. Nothing was sent and no outcome follows.
    Busy,
    /// The request is above the service's bound. Nothing was sent and no outcome follows.
    TooLarge,
}

impl CallStatus {
    /// Its `HYPHA_CALL_*` code.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Sent => HYPHA_CALL_SENT,
            Self::Busy => HYPHA_CALL_BUSY,
            Self::TooLarge => HYPHA_CALL_TOO_LARGE,
        }
    }
}

/// The one outcome of a call that was sent. Each text is the reason `describe()` gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallOutcome {
    /// The server replied with this response.
    Answered(Vec<u8>),
    /// The server refused, for this reason.
    Refused(String),
    /// No live server of the stream until the timeout; the text says how long and where
    /// the timeout came from, and names any other version served on the name.
    NoServer(String),
    /// Nothing was sent, for this reason.
    NotSent(String),
    /// The server crashed or left, failed while handling the call, or its answer did not
    /// verify, for this reason.
    ServerLost(String),
    /// No answer within the timeout; the text says how long and where it came from.
    Timeout(String),
}

impl CallOutcome {
    /// Its `HYPHA_CALL_*` code.
    #[must_use]
    pub const fn code(&self) -> u32 {
        match self {
            Self::Answered(_) => HYPHA_CALL_ANSWERED,
            Self::Refused(_) => HYPHA_CALL_REFUSED,
            Self::NoServer(_) => HYPHA_CALL_NO_SERVER,
            Self::NotSent(_) => HYPHA_CALL_NOT_SENT,
            Self::ServerLost(_) => HYPHA_CALL_SERVER_LOST,
            Self::Timeout(_) => HYPHA_CALL_TIMEOUT,
        }
    }

    /// Whether the server may have run the request: answered, lost or timed out.
    #[must_use]
    pub const fn may_have_run(&self) -> bool {
        matches!(self, Self::Answered(_) | Self::ServerLost(_) | Self::Timeout(_))
    }

    /// Whether a blind retry is safe: the request certainly did not run.
    #[must_use]
    pub const fn safe_to_retry(&self) -> bool {
        matches!(self, Self::Refused(_) | Self::NoServer(_) | Self::NotSent(_))
    }

    /// The response of an answered call.
    #[must_use]
    pub fn response(&self) -> Option<&[u8]> {
        match self {
            Self::Answered(response) => Some(response),
            _ => None,
        }
    }
}

impl fmt::Display for CallOutcome {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Answered(_) => write!(out, "answered"),
            Self::Refused(why) if why.is_empty() => write!(out, "refused"),
            Self::Refused(why) => write!(out, "refused: {why}"),
            Self::NoServer(why) => write!(out, "no server: {why}"),
            Self::NotSent(why) => write!(out, "not sent: {why}"),
            Self::ServerLost(why) => write!(out, "server lost: {why}"),
            Self::Timeout(why) => write!(out, "timeout {why}"),
        }
    }
}

/// One finished call, as an outcome callback receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallResult {
    /// What became of the call.
    pub outcome: CallOutcome,
    /// The client's framework time when the call was made, in nanoseconds.
    pub sent_nanos: i64,
    /// The client's framework time when the outcome was found.
    pub done_nanos: i64,
}

#[cfg(test)]
mod tests {
    use super::CallOutcome;

    #[test]
    fn each_outcome_says_whether_it_may_have_run_and_whether_a_retry_is_safe() {
        let rows = [
            (CallOutcome::Answered(Vec::new()), true, false, "answered"),
            (CallOutcome::Refused("not armed".into()), false, true, "refused: not armed"),
            (CallOutcome::NoServer("within 1 s".into()), false, true, "no server: within 1 s"),
            (CallOutcome::NotSent("x".into()), false, true, "not sent: x"),
            (
                CallOutcome::ServerLost("the server left".into()),
                true,
                false,
                "server lost: the server left",
            ),
            (CallOutcome::Timeout("after 1 s".into()), true, false, "timeout after 1 s"),
        ];
        for (outcome, may_have_run, safe_to_retry, text) in rows {
            assert_eq!(outcome.may_have_run(), may_have_run, "{outcome}");
            assert_eq!(outcome.safe_to_retry(), safe_to_retry, "{outcome}");
            assert_eq!(outcome.to_string(), text);
        }
    }
}
