//! The keys of an input port table: allowed set, declared limits, ranges and `ignored`.
#![allow(
    clippy::expect_used,
    clippy::unnecessary_wraps,
    reason = "test setup must succeed; the expected-result helpers mirror the Result they compare"
)]

use std::sync::atomic::{AtomicUsize, Ordering};

use hypha_core::introspection::{Identity, IntrospectionRegistry};
use hypha_core::parameters::Parameters;
use hypha_runtime::port_limits::{
    Declared, Port, PortError, Supervision, SupervisionLimits, SupervisionRanges, supervise,
};

const MILLI: i64 = 1_000_000;
const HOUR: i64 = 3_600_000 * MILLI;

/// The range a port limit must fall in: one millisecond to one hour.
const RANGES: SupervisionRanges = SupervisionRanges {
    silence_min_nanos: MILLI,
    silence_max_nanos: HOUR,
    stagnation_min_nanos: MILLI,
    stagnation_max_nanos: HOUR,
};

/// A range that only the i64 overflow of a millisecond value can break.
const WIDE_RANGES: SupervisionRanges = SupervisionRanges {
    silence_min_nanos: MILLI,
    silence_max_nanos: i64::MAX,
    stagnation_min_nanos: MILLI,
    stagnation_max_nanos: i64::MAX,
};

/// Silence keeps the wide range of `RANGES`. Stagnation accepts only 10 ms to 500 ms.
const NARROW_STAGNATION: SupervisionRanges = SupervisionRanges {
    silence_min_nanos: MILLI,
    silence_max_nanos: HOUR,
    stagnation_min_nanos: 10 * MILLI,
    stagnation_max_nanos: 500 * MILLI,
};

/// Every value of a millisecond key that fits an i64 of nanoseconds.
const FULL_RANGE: SupervisionRanges = SupervisionRanges {
    silence_min_nanos: 0,
    silence_max_nanos: i64::MAX,
    stagnation_min_nanos: 0,
    stagnation_max_nanos: i64::MAX,
};

// A required port needs `silence_ms`; an advisory port may set it.
const BOTH: Declared = Declared { advisory: false, unsupervised: false, stagnation: true };
const SILENCE: Declared = Declared { advisory: false, unsupervised: false, stagnation: false };
const STAGNATION: Declared = Declared { advisory: true, unsupervised: false, stagnation: true };
const NONE: Declared = Declared { advisory: true, unsupervised: false, stagnation: false };

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Load `source` through the production file loader.
fn parameters(source: &str) -> Parameters {
    let dir = std::env::temp_dir().join(format!(
        "hypha_runtime_port_limits_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let path = dir.join("params.toml");
    std::fs::write(&path, source).expect("write fixture file");
    let loaded = Parameters::from_toml_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
    loaded.expect("valid parameter text")
}

/// One call of `supervise` on the input port `name` of `source`.
struct Call<'a> {
    source: &'a str,
    name: &'a str,
    declared: Declared,
    takes_topic: bool,
    read_limits: bool,
    ranges: SupervisionRanges,
}

impl Call<'_> {
    fn on(source: &'static str, name: &'static str) -> Call<'static> {
        Call { source, name, declared: BOTH, takes_topic: true, read_limits: true, ranges: RANGES }
    }

    fn declared(self, declared: Declared) -> Self {
        Self { declared, ..self }
    }

    fn source_port(self) -> Self {
        Self { takes_topic: false, ..self }
    }

    fn disabled(self) -> Self {
        Self { read_limits: false, ..self }
    }

    fn ranges(self, ranges: SupervisionRanges) -> Self {
        Self { ranges, ..self }
    }

    fn run(self) -> Result<Supervision, PortError> {
        let parameters = parameters(self.source);
        let introspection = IntrospectionRegistry::new(Identity::default());
        let entry = format!("ports.inputs.{}", self.name);
        let port = Port { parameters: &parameters, introspection: &introspection, entry: &entry };
        supervise(port, self.declared, self.takes_topic, self.read_limits, &self.ranges)
    }
}

fn invalid(text: &str) -> Result<Supervision, PortError> {
    Err(PortError::Invalid(text.to_owned()))
}

fn silence(nanos: i64) -> Result<Supervision, PortError> {
    Ok(Supervision {
        limits: SupervisionLimits { silence_nanos: nanos, stagnation_nanos: 0 },
        ..Supervision::default()
    })
}

const KEYS: &str = r#"[ports.inputs.plain]
topic = "/plain"
ignored = true

[ports.inputs.timed]
topic = "/timed"
silence_ms = 250

[ports.inputs.stalled]
topic = "/stalled"
stagnation_ms = 500

[ports.inputs.colour_port]
topic = "/colour"
colour = "red"

[ports.inputs.two_unknown]
topic = "/two"
zeta = 1
alpha = 2

[ports.inputs.nested]
topic = "/nested"
extra.deep = 3

[ports.inputs.unlimited]
topic = "/unlimited"

[ports.inputs.at_min]
topic = "/at_min"
silence_ms = 1

[ports.inputs.at_max]
topic = "/at_max"
silence_ms = 3600000

[ports.inputs.too_fast]
topic = "/too_fast"
silence_ms = 0

[ports.inputs.too_slow]
topic = "/too_slow"
silence_ms = 3600001

[ports.inputs.huge]
topic = "/huge"
silence_ms = 9300000000000

[ports.inputs.wrong_type]
topic = "/wrong_type"
silence_ms = "250"

[ports.inputs.flag_text]
topic = "/flag_text"
silence_ms = 250
ignored = "yes"

[ports.inputs.flagged]
topic = "/flagged"
silence_ms = 250
ignored = true

[ports.inputs.source_limit]
silence_ms = 250

[ports.inputs.source_topic]
topic = "/source"
silence_ms = 250

[ports.inputs.policy_oldest]
topic = "/oldest"
overflow_policy = "drop_oldest"

"#;

#[test]
fn a_table_with_only_allowed_keys_or_no_table_has_no_unknown_key() {
    assert_eq!(Call::on(KEYS, "plain").declared(NONE).disabled().run(), Ok(Supervision::default()));
    assert_eq!(
        Call::on(KEYS, "absent").declared(NONE).disabled().run(),
        Ok(Supervision::default())
    );
}

#[test]
fn silence_ms_is_a_known_key_of_every_input() {
    assert!(Call::on(KEYS, "timed").declared(NONE).disabled().run().is_ok());
    assert!(Call::on(KEYS, "timed").declared(SILENCE).disabled().run().is_ok());
}

#[test]
fn stagnation_ms_is_refused_unless_it_is_declared() {
    assert_eq!(
        Call::on(KEYS, "stalled").declared(SILENCE).disabled().run(),
        invalid("unknown key 'stagnation_ms'")
    );
    assert!(Call::on(KEYS, "stalled").declared(STAGNATION).disabled().run().is_ok());
}

#[test]
fn the_first_unknown_key_in_sorted_order_is_reported_as_one_dotted_name() {
    assert_eq!(Call::on(KEYS, "colour_port").run(), invalid("unknown key 'colour'"));
    assert_eq!(
        Call::on(KEYS, "two_unknown").declared(NONE).disabled().run(),
        invalid("unknown key 'alpha'")
    );
    assert_eq!(
        Call::on(KEYS, "nested").declared(NONE).disabled().run(),
        invalid("unknown key 'extra.deep'")
    );
}

#[test]
fn a_declared_limit_that_is_absent_is_missing() {
    assert_eq!(
        Call::on(KEYS, "unlimited").declared(SILENCE).run(),
        Err(PortError::Missing(hypha_runtime::port_limits::MISSING_SILENCE.to_owned()))
    );
    assert_eq!(
        Call::on(KEYS, "stalled").run(),
        Err(PortError::Missing(hypha_runtime::port_limits::MISSING_SILENCE.to_owned()))
    );
}

#[test]
fn a_limit_of_another_type_is_refused() {
    assert_eq!(
        Call::on(KEYS, "wrong_type").declared(SILENCE).run(),
        invalid("key 'silence_ms' has another type")
    );
}

#[test]
fn a_limit_outside_its_range_is_refused_and_not_clamped() {
    let outside = invalid("key 'silence_ms' is outside its range");
    assert_eq!(Call::on(KEYS, "too_fast").declared(SILENCE).run(), outside);
    assert_eq!(Call::on(KEYS, "too_slow").declared(SILENCE).run(), outside);
    assert_eq!(Call::on(KEYS, "huge").declared(SILENCE).ranges(WIDE_RANGES).run(), outside);
}

#[test]
fn a_limit_exactly_at_its_minimum_or_maximum_is_accepted_in_nanoseconds() {
    assert_eq!(Call::on(KEYS, "at_min").declared(SILENCE).run(), silence(MILLI));
    assert_eq!(Call::on(KEYS, "at_max").declared(SILENCE).run(), silence(HOUR));
    assert_eq!(Call::on(KEYS, "timed").declared(SILENCE).run(), silence(250 * MILLI));
    assert_eq!(
        Call::on(KEYS, "stalled").declared(STAGNATION).run(),
        Ok(Supervision {
            limits: SupervisionLimits { silence_nanos: 0, stagnation_nanos: 500 * MILLI },
            ..Supervision::default()
        })
    );
}

#[test]
fn ignored_is_false_when_absent_and_reported_when_true() {
    assert_eq!(Call::on(KEYS, "timed").declared(SILENCE).run().map(|s| s.ignored), Ok(false));
    assert_eq!(Call::on(KEYS, "flagged").declared(SILENCE).run().map(|s| s.ignored), Ok(true));
}

#[test]
fn ignored_on_a_port_without_a_limit_or_of_another_type_is_refused() {
    assert_eq!(
        Call::on(KEYS, "plain").declared(NONE).run(),
        invalid("key 'ignored' needs a silence_ms or stagnation_ms limit")
    );
    assert_eq!(
        Call::on(KEYS, "flag_text").declared(SILENCE).run(),
        invalid("key 'ignored' has another type")
    );
}

#[test]
fn a_source_port_refuses_a_topic_key_and_reads_its_limit_without_one() {
    let refused = invalid("a port that reads a Source takes no topic");
    assert_eq!(Call::on(KEYS, "source_topic").declared(SILENCE).source_port().run(), refused);
    assert_eq!(Call::on(KEYS, "plain").declared(NONE).source_port().run(), refused);
    assert_eq!(
        Call::on(KEYS, "source_limit").declared(SILENCE).source_port().run(),
        silence(250 * MILLI)
    );
}

#[test]
fn a_disabled_port_checks_its_keys_but_reads_no_limit_or_flag() {
    assert_eq!(
        Call::on(KEYS, "unlimited").declared(SILENCE).disabled().run(),
        Ok(Supervision::default())
    );
    assert_eq!(
        Call::on(KEYS, "plain").declared(SILENCE).disabled().run(),
        Ok(Supervision::default())
    );
    assert_eq!(
        Call::on(KEYS, "colour_port").declared(SILENCE).disabled().run(),
        invalid("unknown key 'colour'")
    );
}

#[test]
fn a_port_with_no_declared_check_reads_no_limit_and_no_flag() {
    assert_eq!(Call::on(KEYS, "unlimited").declared(NONE).run(), Ok(Supervision::default()));
}

#[test]
fn an_overflow_policy_key_is_unknown() {
    assert_eq!(
        Call::on(KEYS, "policy_oldest").declared(NONE).run(),
        invalid("unknown key 'overflow_policy'")
    );
}

const BOUNDS: &str = r#"[ports.inputs.stag_low]
topic = "/a"
stagnation_ms = 9

[ports.inputs.stag_min]
topic = "/b"
stagnation_ms = 10

[ports.inputs.stag_max]
topic = "/c"
stagnation_ms = 500

[ports.inputs.stag_high]
topic = "/d"
stagnation_ms = 501

[ports.inputs.stag_wide]
topic = "/e"
stagnation_ms = 2000

[ports.inputs.silence_wide]
topic = "/f"
silence_ms = 2000

[ports.inputs.huge_max]
topic = "/g"
silence_ms = 9223372036854

[ports.inputs.huge_over]
topic = "/h"
silence_ms = 9223372036855

[ports.inputs.wrapped]
topic = "/i"
silence_ms = 18446744073711
"#;

fn stagnation(nanos: i64) -> Result<Supervision, PortError> {
    Ok(Supervision {
        limits: SupervisionLimits { silence_nanos: 0, stagnation_nanos: nanos },
        ..Supervision::default()
    })
}

#[test]
fn a_stagnation_limit_is_checked_against_its_own_range_only() {
    let narrow = |name| Call::on(BOUNDS, name).declared(STAGNATION).ranges(NARROW_STAGNATION).run();
    let outside = invalid("key 'stagnation_ms' is outside its range");
    assert_eq!(narrow("stag_low"), outside);
    assert_eq!(narrow("stag_min"), stagnation(10 * MILLI));
    assert_eq!(narrow("stag_max"), stagnation(500 * MILLI));
    assert_eq!(narrow("stag_high"), outside);
    assert_eq!(narrow("stag_wide"), outside, "a value valid for silence is refused as stagnation");
    assert_eq!(
        Call::on(BOUNDS, "silence_wide").declared(SILENCE).ranges(NARROW_STAGNATION).run(),
        silence(2000 * MILLI),
        "silence is checked against the silence range only"
    );
}

#[test]
fn a_millisecond_value_whose_nanoseconds_wrap_is_refused_and_the_largest_is_accepted() {
    let full = |name| Call::on(BOUNDS, name).declared(SILENCE).ranges(FULL_RANGE).run();
    let outside = invalid("key 'silence_ms' is outside its range");
    assert_eq!(full("huge_over"), outside);
    assert_eq!(full("wrapped"), outside);
    assert_eq!(full("huge_max"), silence(9_223_372_036_854_000_000));
}

/// One port with `silence_ms` and one without.
const RULE: &str =
    "[ports.inputs.limited]\nsilence_ms = 200\n[ports.inputs.bare]\ntopic = \"/bare\"\n";

const REQUIRED: Declared = Declared { advisory: false, unsupervised: false, stagnation: false };
const ADVISORY: Declared = Declared { advisory: true, unsupervised: false, stagnation: false };
const UNSUPERVISED: Declared = Declared { advisory: false, unsupervised: true, stagnation: false };
const BOTH_TAGS: Declared = Declared { advisory: true, unsupervised: true, stagnation: false };

fn silence_of(result: &Result<Supervision, PortError>) -> Option<i64> {
    result.as_ref().ok().map(|found| found.limits.silence_nanos)
}

#[test]
fn the_rule_is_decided_from_advisory_and_unsupervised() {
    // A required input: the key is required, and read when present.
    let required = Call::on(RULE, "bare").declared(REQUIRED).run();
    assert_eq!(
        required,
        Err(PortError::Missing(hypha_runtime::port_limits::MISSING_SILENCE.to_owned()))
    );
    assert_eq!(silence_of(&Call::on(RULE, "limited").declared(REQUIRED).run()), Some(200 * MILLI));
    // An advisory input: optional, read when present.
    assert_eq!(silence_of(&Call::on(RULE, "bare").declared(ADVISORY).run()), Some(0));
    assert_eq!(silence_of(&Call::on(RULE, "limited").declared(ADVISORY).run()), Some(200 * MILLI));
    // An unsupervised input: never a limit, and the key is refused.
    assert_eq!(silence_of(&Call::on(RULE, "bare").declared(UNSUPERVISED).run()), Some(0));
    assert_eq!(
        Call::on(RULE, "limited").declared(UNSUPERVISED).run(),
        invalid("key 'silence_ms' on a port tagged unsupervised")
    );
    // Both tags exclude each other, with or without the key.
    for name in ["bare", "limited"] {
        assert_eq!(
            Call::on(RULE, name).declared(BOTH_TAGS).run(),
            invalid("the tags unsupervised and advisory exclude each other")
        );
    }
}

#[test]
fn a_disabled_input_needs_no_limit() {
    assert_eq!(silence_of(&Call::on(RULE, "bare").declared(REQUIRED).disabled().run()), Some(0));
}
