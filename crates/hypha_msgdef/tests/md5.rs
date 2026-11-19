//! MD5: the RFC 1321 known-answer suite, padding boundaries and ROS 1 message identity.

use std::fmt::Write as _;

use hypha_msgdef::definition::parse_definition;
use hypha_msgdef::md5::{md5_from_definition, md5_hex};
use hypha_msgdef::resolve::{ResolveError, message_identity};
use hypha_msgdef::spec::{MAX_NESTING_DEPTH, TypeName};

#[test]
fn empty_string() {
    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn single_char() {
    assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
}

#[test]
fn abc() {
    assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
}

#[test]
fn message_digest_phrase() {
    assert_eq!(md5_hex(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
}

#[test]
fn lowercase_alphabet() {
    assert_eq!(md5_hex(b"abcdefghijklmnopqrstuvwxyz"), "c3fcd3d76192e4007dfb496cca67e13b");
}

#[test]
fn mixed_case_alphanumeric() {
    assert_eq!(
        md5_hex(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
        "d174ab98d277d9f5a5611c2c9f419d9f"
    );
}

#[test]
fn eighty_digit_string() {
    assert_eq!(
        md5_hex(
            b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
        ),
        "57edf4a22be3c955ac49da2e2107b67a"
    );
}

/// A message of exactly `length` bytes, to probe the MD5 padding boundaries.
fn message_of_length(length: usize) -> Vec<u8> {
    let alphabet = b"abcdefghijklmnopqrstuvwxyz";
    let mut message: Vec<u8> = Vec::with_capacity(length);
    while message.len() < length {
        let alphabet_index = message.len().checked_rem(alphabet.len()).unwrap_or(0);
        let Some(byte) = alphabet.get(alphabet_index) else {
            break;
        };
        message.push(*byte);
    }
    message
}

// The expected digests below were computed independently with Python's hashlib.

/// Concatenated definition text: the first entry is the root body; every later entry
/// becomes a `MSG: package/Type` section.
fn build_definition(entries: &[(&str, &str)]) -> String {
    let mut text = String::new();
    let mut is_first = true;
    for (name, body) in entries {
        if is_first {
            text.push_str(body);
            is_first = false;
            continue;
        }
        text.push('\n');
        text.push_str(
            "================================================================================\n",
        );
        text.push_str("MSG: ");
        text.push_str(name);
        text.push('\n');
        text.push_str(body);
    }
    text
}

const HEADER_BODY: &str = "uint32 seq\ntime stamp\nstring frame_id";
const POINT_BODY: &str = "float64 x\nfloat64 y\nfloat64 z";
const VECTOR3_BODY: &str = "float64 x\nfloat64 y\nfloat64 z";
const QUATERNION_BODY: &str = "float64 x\nfloat64 y\nfloat64 z\nfloat64 w";
const POSE_BODY: &str = "Point position\nQuaternion orientation";
const TRANSFORM_BODY: &str = "Vector3 translation\nQuaternion rotation";
const TRANSFORM_STAMPED_BODY: &str = "Header header\nstring child_frame_id\nTransform transform";

#[test]
fn padding_boundaries_match_independent_digests() {
    let cases = [
        (55, "0d7ae056b2f015cd7dc67494efd658f1"),
        (56, "31fcfb5165169eb55898e7e4cf34d19a"),
        (63, "1b30c0670c15e7da3c2ba7bce77ebe99"),
        (64, "a2eaf6295c32adc403865fd96a2f182b"),
        (65, "eba2cce0ca8df47e62414a736b3105a2"),
    ];
    for (length, digest) in cases {
        assert_eq!(md5_hex(&message_of_length(length)), digest, "{length} bytes");
    }
}

#[test]
fn std_msgs_header() {
    let text = build_definition(&[("std_msgs/Header", HEADER_BODY)]);
    let hash = md5_from_definition("std_msgs/Header", &text);
    assert_eq!(hash.as_deref(), Some("2176decaecbce78abc3b96ef049fabed"));
}

#[test]
fn std_msgs_bool() {
    let text = build_definition(&[("std_msgs/Bool", "bool data")]);
    let hash = md5_from_definition("std_msgs/Bool", &text);
    assert_eq!(hash.as_deref(), Some("8b94c1b53db61fb6aed406028ad6332a"));
}

#[test]
fn std_msgs_string() {
    let text = build_definition(&[("std_msgs/String", "string data")]);
    let hash = md5_from_definition("std_msgs/String", &text);
    assert_eq!(hash.as_deref(), Some("992ce8a1687cec8c8bd883ec73ca41d1"));
}

#[test]
fn std_msgs_float64() {
    let text = build_definition(&[("std_msgs/Float64", "float64 data")]);
    let hash = md5_from_definition("std_msgs/Float64", &text);
    assert_eq!(hash.as_deref(), Some("fdb28210bfa9d7c91146260178d9a584"));
}

#[test]
fn geometry_msgs_point() {
    let text = build_definition(&[("geometry_msgs/Point", POINT_BODY)]);
    let hash = md5_from_definition("geometry_msgs/Point", &text);
    assert_eq!(hash.as_deref(), Some("4a842b65f413084dc2b10fb484ea7f17"));
}

#[test]
fn geometry_msgs_vector3() {
    let text = build_definition(&[("geometry_msgs/Vector3", VECTOR3_BODY)]);
    let hash = md5_from_definition("geometry_msgs/Vector3", &text);
    assert_eq!(hash.as_deref(), Some("4a842b65f413084dc2b10fb484ea7f17"));
}

#[test]
fn geometry_msgs_quaternion() {
    let text = build_definition(&[("geometry_msgs/Quaternion", QUATERNION_BODY)]);
    let hash = md5_from_definition("geometry_msgs/Quaternion", &text);
    assert_eq!(hash.as_deref(), Some("a779879fadf0160734f906b8c19c7004"));
}

#[test]
fn geometry_msgs_pose() {
    let text = build_definition(&[
        ("geometry_msgs/Pose", POSE_BODY),
        ("geometry_msgs/Point", POINT_BODY),
        ("geometry_msgs/Quaternion", QUATERNION_BODY),
    ]);
    let hash = md5_from_definition("geometry_msgs/Pose", &text);
    assert_eq!(hash.as_deref(), Some("e45d45a5a1ce597b249e23fb30fc871f"));
}

#[test]
fn geometry_msgs_pose_stamped() {
    let text = build_definition(&[
        ("geometry_msgs/PoseStamped", "Header header\nPose pose"),
        ("std_msgs/Header", HEADER_BODY),
        ("geometry_msgs/Pose", POSE_BODY),
        ("geometry_msgs/Point", POINT_BODY),
        ("geometry_msgs/Quaternion", QUATERNION_BODY),
    ]);
    let hash = md5_from_definition("geometry_msgs/PoseStamped", &text);
    assert_eq!(hash.as_deref(), Some("d3812c3cbc69362b77dc0b19b345f8f5"));
}

#[test]
fn geometry_msgs_transform() {
    let text = build_definition(&[
        ("geometry_msgs/Transform", TRANSFORM_BODY),
        ("geometry_msgs/Vector3", VECTOR3_BODY),
        ("geometry_msgs/Quaternion", QUATERNION_BODY),
    ]);
    let hash = md5_from_definition("geometry_msgs/Transform", &text);
    assert_eq!(hash.as_deref(), Some("ac9eff44abf714214112b05d54a3cf9b"));
}

#[test]
fn geometry_msgs_transform_stamped() {
    let text = build_definition(&[
        ("geometry_msgs/TransformStamped", TRANSFORM_STAMPED_BODY),
        ("std_msgs/Header", HEADER_BODY),
        ("geometry_msgs/Transform", TRANSFORM_BODY),
        ("geometry_msgs/Vector3", VECTOR3_BODY),
        ("geometry_msgs/Quaternion", QUATERNION_BODY),
    ]);
    let hash = md5_from_definition("geometry_msgs/TransformStamped", &text);
    assert_eq!(hash.as_deref(), Some("b5764a33bfeb3588febc2682852579b0"));
}

#[test]
fn tf2_msgs_tf_message() {
    let text = build_definition(&[
        ("tf2_msgs/TFMessage", "geometry_msgs/TransformStamped[] transforms"),
        ("geometry_msgs/TransformStamped", TRANSFORM_STAMPED_BODY),
        ("std_msgs/Header", HEADER_BODY),
        ("geometry_msgs/Transform", TRANSFORM_BODY),
        ("geometry_msgs/Vector3", VECTOR3_BODY),
        ("geometry_msgs/Quaternion", QUATERNION_BODY),
    ]);
    let hash = md5_from_definition("tf2_msgs/TFMessage", &text);
    assert_eq!(hash.as_deref(), Some("94810edda583a504dfda3829e70d7eec"));
}

#[test]
fn std_msgs_empty() {
    assert_eq!(
        md5_from_definition("std_msgs/Empty", ""),
        Some(String::from("d41d8cd98f00b204e9800998ecf8427e"))
    );
}

#[test]
fn missing_nested_section_returns_none() {
    let hash = md5_from_definition("geometry_msgs/PoseStamped", "geometry_msgs/Point position");
    assert_eq!(hash, None);
}

#[test]
fn comments_and_blank_lines_do_not_change_the_hash() {
    let text = build_definition(&[(
        "std_msgs/Header",
        "# a header field\nuint32 seq  # sequence id\n\ntime stamp\n   \nstring frame_id # frame\n",
    )]);
    let hash = md5_from_definition("std_msgs/Header", &text);
    assert_eq!(hash.as_deref(), Some("2176decaecbce78abc3b96ef049fabed"));
}

#[test]
fn header_and_fully_qualified_header_produce_the_same_hash() {
    let short_text = build_definition(&[
        ("geometry_msgs/PoseStamped", "Header header\nPose pose"),
        ("std_msgs/Header", HEADER_BODY),
        ("geometry_msgs/Pose", POSE_BODY),
        ("geometry_msgs/Point", POINT_BODY),
        ("geometry_msgs/Quaternion", QUATERNION_BODY),
    ]);
    let qualified_text = build_definition(&[
        ("geometry_msgs/PoseStamped", "std_msgs/Header header\nPose pose"),
        ("std_msgs/Header", HEADER_BODY),
        ("geometry_msgs/Pose", POSE_BODY),
        ("geometry_msgs/Point", POINT_BODY),
        ("geometry_msgs/Quaternion", QUATERNION_BODY),
    ]);
    let short_hash = md5_from_definition("geometry_msgs/PoseStamped", &short_text);
    let qualified_hash = md5_from_definition("geometry_msgs/PoseStamped", &qualified_text);
    assert_eq!(short_hash, qualified_hash);
    assert_eq!(short_hash.as_deref(), Some("d3812c3cbc69362b77dc0b19b345f8f5"));
}

#[test]
fn crlf_line_endings_are_handled() {
    let body = "uint32 seq\r\ntime stamp\r\nstring frame_id\r\n";
    let text = build_definition(&[("std_msgs/Header", body)]);
    let hash = md5_from_definition("std_msgs/Header", &text);
    assert_eq!(hash.as_deref(), Some("2176decaecbce78abc3b96ef049fabed"));
}

#[test]
fn constant_line_is_handled_and_sorts_before_fields() {
    let text = build_definition(&[("test_msgs/WithConstant", "int32 data\nint32 FOO=5")]);
    let with_constant_first = md5_from_definition("test_msgs/WithConstant", &text);

    let reordered_text = build_definition(&[("test_msgs/WithConstant", "int32 FOO=5\nint32 data")]);
    let with_constant_second = md5_from_definition("test_msgs/WithConstant", &reordered_text);

    assert_eq!(with_constant_first, with_constant_second);

    let expected = md5_hex(b"int32 FOO=5\nint32 data");
    assert_eq!(with_constant_first.as_deref(), Some(expected.as_str()));
}

#[test]
fn a_cyclic_definition_has_no_md5() {
    let text = build_definition(&[("pkg/A", "pkg/B b"), ("pkg/B", "pkg/A a")]);
    assert_eq!(md5_from_definition("pkg/A", &text), None);
}

#[test]
fn a_definition_the_parser_rejects_has_no_md5() {
    assert_eq!(md5_from_definition("pkg/Bad", "int32[ data"), None);
    assert_eq!(md5_from_definition("no_package", "int32 data"), None);
}

/// Reference values from `rosbags` 0.11.9 (`Typestore.generate_msgdef`, ROS1), which
/// matches genmsg: a string constant keeps `#` but loses surrounding whitespace.
#[test]
fn string_constants_hash_like_ros1_rosbags() {
    let cases = [
        ("string FOO=bar # not a comment\nint32 x", "091deebdeb5a6a484cd71e15082f7a87"),
        ("string FOO=  padded  \nint32 x", "2f18eb9c821239e1a3c08fa74d5d2f03"),
        ("int32 BAR=5 # comment\nint32 x", "5840be4337b302b0e27f924df0b6951f"),
    ];
    for (text, expected) in cases {
        assert_eq!(md5_from_definition("a/Case", text).as_deref(), Some(expected), "{text}");
    }
}

/// A definition of `depth` levels below the root, `pkg/T0` to `pkg/T{depth}`;
/// every level holds the next one through `fields` (one field line per name).
fn nested_definition(depth: usize, fields: &[&str]) -> String {
    let separator = "=".repeat(80);
    let holds = |next: usize| {
        fields.iter().map(|name| format!("pkg/T{next} {name}")).collect::<Vec<_>>().join("\n")
            + "\n"
    };
    let mut sections = vec![holds(0)];
    for (level, next) in (0..depth).zip(1..) {
        sections.push(format!("{separator}\nMSG: pkg/T{level}\n{}", holds(next)));
    }
    sections.push(format!("{separator}\nMSG: pkg/T{depth}\nint32 v\n"));
    sections.concat()
}

/// Each shared type is hashed once: a diamond of 14 levels is hashed, and one of 40
/// levels is refused at once because it expands past `MAX_EXPANDED_MESSAGES`.
#[test]
#[cfg_attr(miri, ignore = "a wall-clock bound (100 ms) that an interpreter cannot meet")]
fn a_type_shared_by_many_paths_is_hashed_once() {
    assert!(md5_from_definition("pkg/Root", &nested_definition(13, &["a", "b"])).is_some());
    let started = std::time::Instant::now();
    assert!(md5_from_definition("pkg/Root", &nested_definition(40, &["a", "b"])).is_none());
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
}

/// The identity of `definition_text` as `type_name`, computed by the resolver so the refusal
/// keeps its reason.
#[allow(clippy::expect_used)]
fn resolver_result(type_name: &str, definition_text: &str) -> Result<String, ResolveError> {
    let (package, name) = type_name.split_once('/').expect("type name has a package");
    let root = TypeName::new(package, name);
    let specs = parse_definition(&root, definition_text).expect("definition parses");
    message_identity(&specs, &root)
}

/// `pkg/Root` holds `pkg/T{j}` for every multiple of 60 below `levels`, descending, and
/// `pkg/T{i}` holds `pkg/T{i+1}`, the last holding an `int32`.
fn shared_chain_definition(levels: usize) -> String {
    let separator = "=".repeat(80);
    let shared: Vec<usize> = (0..levels).step_by(60).rev().collect();
    let mut root = String::new();
    for index in shared {
        let _ = writeln!(root, "pkg/T{index} f{index}");
    }
    let mut sections = vec![root];
    for level in 0..levels {
        let next = level.saturating_add(1);
        let body = if next < levels {
            format!("pkg/T{next} child\n")
        } else {
            String::from("int32 value\n")
        };
        sections.push(format!("{separator}\nMSG: pkg/T{level}\n{body}"));
    }
    sections.concat()
}

/// A deep, linear definition is refused at the nesting limit without overflowing the stack.
#[test]
#[cfg_attr(miri, ignore = "a scale test: ten thousand levels take Miri more than ten minutes")]
fn a_ten_thousand_level_definition_is_refused_at_the_nesting_limit() {
    // Root is level 1 and pkg/T{k} is level k + 2, so the first type past the limit is T63.
    let result = resolver_result("pkg/Root", &nested_definition(10_000, &["x"]));
    assert_eq!(
        result,
        Err(ResolveError::NestingTooDeep {
            limit: MAX_NESTING_DEPTH,
            at: TypeName::new("pkg", "T63")
        })
    );
}

/// A type first reached on a shallow path has its deep path checked too.
#[test]
fn a_shared_chain_past_the_nesting_limit_has_no_md5() {
    let definition = shared_chain_definition(200);
    assert_eq!(md5_from_definition("pkg/Root", &definition), None);
    assert!(matches!(
        resolver_result("pkg/Root", &definition),
        Err(ResolveError::NestingTooDeep { limit: MAX_NESTING_DEPTH, .. })
    ));
}
