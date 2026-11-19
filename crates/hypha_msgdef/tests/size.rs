//! The fixed encoded size of a verified definition, and its refusals.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

use std::fmt::Write as _;

use hypha_msgdef::md5::md5_from_definition;
use hypha_msgdef::size::{EncodedSize, SizeError, verified_encoded_size};
use hypha_msgdef::spec::MAX_NESTING_DEPTH;

const SEP: &str =
    "\n================================================================================\n";

fn size_of(type_name: &str, definition: &str) -> Result<EncodedSize, SizeError> {
    let md5 = md5_from_definition(type_name, definition).expect("the definition is complete");
    verified_encoded_size(type_name, &md5, definition)
}

#[test]
fn primitives_time_and_duration_are_fixed() {
    assert_eq!(size_of("rosgraph_msgs/Clock", "time clock"), Ok(EncodedSize::Fixed(8)));
    let all = "bool a\nbyte b\nchar c\nint8 d\nuint8 e\nint16 f\nuint16 g\nint32 h\nuint32 i\n\
               float32 j\nint64 k\nuint64 l\nfloat64 m\ntime n\nduration o\nint32 CONST=3";
    assert_eq!(size_of("p/All", all), Ok(EncodedSize::Fixed(5 + 4 + 12 + 8 * 5)));
}

#[test]
fn nested_fixed_arrays_multiply() {
    let definition = format!(
        "geometry_msgs/Point[3] corners\nfloat64[9] covariance{SEP}MSG: geometry_msgs/Point\n\
         float64 x\nfloat64 y\nfloat64 z"
    );
    assert_eq!(size_of("p/Box", &definition), Ok(EncodedSize::Fixed(3 * 24 + 9 * 8)));
}

#[test]
fn an_empty_message_is_zero_bytes() {
    assert_eq!(size_of("std_msgs/Empty", ""), Ok(EncodedSize::Fixed(0)));
}

#[test]
fn strings_dynamic_arrays_and_header_types_vary() {
    assert_eq!(size_of("std_msgs/String", "string data"), Ok(EncodedSize::Variable));
    assert_eq!(size_of("p/List", "float32[] values"), Ok(EncodedSize::Variable));
    let stamped = format!(
        "std_msgs/Header header\nfloat64 x{SEP}MSG: std_msgs/Header\n\
         uint32 seq\ntime stamp\nstring frame_id"
    );
    assert_eq!(size_of("p/Stamped", &stamped), Ok(EncodedSize::Variable));
}

#[test]
fn a_definition_of_another_type_is_refused() {
    let wrong = md5_from_definition("p/A", "int32 a").expect("complete");
    assert!(matches!(
        verified_encoded_size("p/A", &wrong, "int64 a"),
        Err(SizeError::Md5Mismatch { .. })
    ));
}

/// A chain of `levels` types, `p/T0` first, the last holding an `int32`.
fn chain_definition(levels: usize) -> String {
    let mut text = String::new();
    for level in 0..levels {
        if level > 0 {
            text.push_str(SEP);
            let _ = writeln!(text, "MSG: p/T{level}");
        }
        let next = level + 1;
        if next < levels {
            let _ = write!(text, "p/T{next} child");
        } else {
            text.push_str("int32 value");
        }
        text.push('\n');
    }
    text
}

#[test]
fn a_chain_at_the_nesting_limit_has_a_size_and_one_level_more_is_refused() {
    let at_limit = chain_definition(MAX_NESTING_DEPTH);
    assert_eq!(size_of("p/T0", &at_limit), Ok(EncodedSize::Fixed(4)));
    let too_deep = chain_definition(MAX_NESTING_DEPTH + 1);
    let md5 = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        verified_encoded_size("p/T0", md5, &too_deep),
        Err(SizeError::NestingTooDeep { limit: MAX_NESTING_DEPTH })
    );
}

#[test]
fn an_incomplete_definition_is_refused() {
    let md5 = "0123456789abcdef0123456789abcdef";
    assert_eq!(verified_encoded_size("p/A", md5, "q/Missing m"), Err(SizeError::Unreadable));
    assert_eq!(verified_encoded_size("NoPackage", md5, "int32 a"), Err(SizeError::Unreadable));
}

/// `p/Root` holds `p/T{j}` for every multiple of 60 below `levels`, descending, and
/// `p/T{i}` holds `p/T{i+1}`, the last holding an `int32`.
fn shared_chain_definition(levels: usize) -> String {
    let shared: Vec<usize> = (0..levels).step_by(60).rev().collect();
    let mut text = String::new();
    for index in shared {
        let _ = writeln!(text, "p/T{index} f{index}");
    }
    for level in 0..levels {
        text.push_str(SEP);
        let _ = writeln!(text, "MSG: p/T{level}");
        let next = level + 1;
        if next < levels {
            let _ = writeln!(text, "p/T{next} child");
        } else {
            text.push_str("int32 value\n");
        }
    }
    text
}

#[test]
fn a_shared_chain_past_the_nesting_limit_has_no_size() {
    // The identity runs before the size walk, so the refusal does not depend on the digest.
    let definition = shared_chain_definition(200);
    assert_eq!(
        verified_encoded_size("p/Root", "0123456789abcdef0123456789abcdef", &definition),
        Err(SizeError::NestingTooDeep { limit: MAX_NESTING_DEPTH })
    );
}

#[test]
#[cfg_attr(miri, ignore = "a scale test: the chain takes Miri more than ten minutes")]
fn a_large_shared_chain_is_refused_without_overflowing_the_stack() {
    let definition = shared_chain_definition(200_000);
    assert_eq!(
        verified_encoded_size("p/Root", "0123456789abcdef0123456789abcdef", &definition),
        Err(SizeError::NestingTooDeep { limit: MAX_NESTING_DEPTH })
    );
}

/// `p/T0` holds two `p/T1`, ..., and `p/T<levels-1>` holds one `int8`.
fn diamond_ladder(levels: usize) -> String {
    let mut text = String::new();
    for level in 0..levels {
        if level > 0 {
            writeln!(text, "{SEP}MSG: p/T{level}").expect("text");
        }
        let last = level + 1 == levels;
        if last {
            text.push_str("int8 x\n");
        } else {
            let next = level + 1;
            write!(text, "p/T{next} a\np/T{next} b\n").expect("text");
        }
    }
    text
}

#[test]
#[cfg_attr(miri, ignore = "a wall-clock bound (100 ms) that an interpreter cannot meet")]
fn a_64_level_diamond_ladder_sizes_in_linear_time() {
    let definition = diamond_ladder(64);
    let started = std::time::Instant::now();
    let result = verified_encoded_size("p/T0", "00000000000000000000000000000000", &definition);
    let took = started.elapsed();
    assert_eq!(result, Err(SizeError::TooLarge));
    assert!(took < std::time::Duration::from_millis(100), "sizing took {took:?}");
}

#[test]
fn a_ladder_that_fits_reports_its_exact_size() {
    assert_eq!(size_of("p/T0", &diamond_ladder(11)), Ok(EncodedSize::Fixed(1024)));
}

#[test]
fn a_fixed_size_past_usize_is_too_large_not_growable() {
    let definition = "uint8[18446744073709551615] a\nuint8[18446744073709551615] b";
    let md5 = md5_from_definition("p/Huge", definition).expect("the definition is complete");
    assert_eq!(verified_encoded_size("p/Huge", &md5, definition), Err(SizeError::TooLarge));
}
