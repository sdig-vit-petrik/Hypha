//! Tests for message dependency resolution and identity calculation.

use std::fmt::Write as _;

use hypha_msgdef::parse::{parse_message, parse_service};
use hypha_msgdef::resolve::{
    ResolveError, SpecSet, definition_text, message_identity, service_identity,
};
use hypha_msgdef::spec::{
    Constant, Field, FieldType, MAX_NESTING_DEPTH, MessageSpec, Multiplicity, TypeName,
};

// The fixture lists the complete supported message graph in definition order.
#[allow(clippy::too_many_lines)]
fn build_full_spec_set() -> SpecSet {
    let mut specs = SpecSet::new();

    specs.insert(MessageSpec {
        type_name: TypeName::new("std_msgs", "Header"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::UInt32,
                multiplicity: Multiplicity::Unit,
                name: "seq".to_string(),
            },
            Field {
                field_type: FieldType::Time,
                multiplicity: Multiplicity::Unit,
                name: "stamp".to_string(),
            },
            Field {
                field_type: FieldType::Text,
                multiplicity: Multiplicity::Unit,
                name: "frame_id".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("std_msgs", "Bool"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Bool,
            multiplicity: Multiplicity::Unit,
            name: "data".to_string(),
        }],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("std_msgs", "String"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Text,
            multiplicity: Multiplicity::Unit,
            name: "data".to_string(),
        }],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("std_msgs", "Float64"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Float64,
            multiplicity: Multiplicity::Unit,
            name: "data".to_string(),
        }],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("geometry_msgs", "Point"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "x".to_string(),
            },
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "y".to_string(),
            },
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "z".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("geometry_msgs", "Vector3"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "x".to_string(),
            },
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "y".to_string(),
            },
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "z".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("geometry_msgs", "Quaternion"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "x".to_string(),
            },
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "y".to_string(),
            },
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "z".to_string(),
            },
            Field {
                field_type: FieldType::Float64,
                multiplicity: Multiplicity::Unit,
                name: "w".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("geometry_msgs", "Pose"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::Message(TypeName::new("geometry_msgs", "Point")),
                multiplicity: Multiplicity::Unit,
                name: "position".to_string(),
            },
            Field {
                field_type: FieldType::Message(TypeName::new("geometry_msgs", "Quaternion")),
                multiplicity: Multiplicity::Unit,
                name: "orientation".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("geometry_msgs", "PoseStamped"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::Message(TypeName::new("std_msgs", "Header")),
                multiplicity: Multiplicity::Unit,
                name: "header".to_string(),
            },
            Field {
                field_type: FieldType::Message(TypeName::new("geometry_msgs", "Pose")),
                multiplicity: Multiplicity::Unit,
                name: "pose".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("geometry_msgs", "Transform"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::Message(TypeName::new("geometry_msgs", "Vector3")),
                multiplicity: Multiplicity::Unit,
                name: "translation".to_string(),
            },
            Field {
                field_type: FieldType::Message(TypeName::new("geometry_msgs", "Quaternion")),
                multiplicity: Multiplicity::Unit,
                name: "rotation".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("geometry_msgs", "TransformStamped"),
        constants: vec![],
        fields: vec![
            Field {
                field_type: FieldType::Message(TypeName::new("std_msgs", "Header")),
                multiplicity: Multiplicity::Unit,
                name: "header".to_string(),
            },
            Field {
                field_type: FieldType::Text,
                multiplicity: Multiplicity::Unit,
                name: "child_frame_id".to_string(),
            },
            Field {
                field_type: FieldType::Message(TypeName::new("geometry_msgs", "Transform")),
                multiplicity: Multiplicity::Unit,
                name: "transform".to_string(),
            },
        ],
    });

    specs.insert(MessageSpec {
        type_name: TypeName::new("tf2_msgs", "TFMessage"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Message(TypeName::new("geometry_msgs", "TransformStamped")),
            multiplicity: Multiplicity::Variable,
            name: "transforms".to_string(),
        }],
    });

    specs
}

#[test]
#[allow(clippy::expect_used)]
fn std_msgs_header_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("std_msgs", "Header"))
        .expect("should compute hash");
    assert_eq!(result, "2176decaecbce78abc3b96ef049fabed");
}

#[test]
#[allow(clippy::expect_used)]
fn std_msgs_bool_identity() {
    let specs = build_full_spec_set();
    let result =
        message_identity(&specs, &TypeName::new("std_msgs", "Bool")).expect("should compute hash");
    assert_eq!(result, "8b94c1b53db61fb6aed406028ad6332a");
}

#[test]
#[allow(clippy::expect_used)]
fn std_msgs_string_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("std_msgs", "String"))
        .expect("should compute hash");
    assert_eq!(result, "992ce8a1687cec8c8bd883ec73ca41d1");
}

#[test]
#[allow(clippy::expect_used)]
fn std_msgs_float64_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("std_msgs", "Float64"))
        .expect("should compute hash");
    assert_eq!(result, "fdb28210bfa9d7c91146260178d9a584");
}

#[test]
#[allow(clippy::expect_used)]
fn geometry_msgs_point_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("geometry_msgs", "Point"))
        .expect("should compute hash");
    assert_eq!(result, "4a842b65f413084dc2b10fb484ea7f17");
}

#[test]
#[allow(clippy::expect_used)]
fn geometry_msgs_vector3_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("geometry_msgs", "Vector3"))
        .expect("should compute hash");
    assert_eq!(result, "4a842b65f413084dc2b10fb484ea7f17");
}

#[test]
#[allow(clippy::expect_used)]
fn geometry_msgs_quaternion_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("geometry_msgs", "Quaternion"))
        .expect("should compute hash");
    assert_eq!(result, "a779879fadf0160734f906b8c19c7004");
}

#[test]
#[allow(clippy::expect_used)]
fn geometry_msgs_pose_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("geometry_msgs", "Pose"))
        .expect("should compute hash");
    assert_eq!(result, "e45d45a5a1ce597b249e23fb30fc871f");
}

#[test]
#[allow(clippy::expect_used)]
fn geometry_msgs_pose_stamped_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("geometry_msgs", "PoseStamped"))
        .expect("should compute hash");
    assert_eq!(result, "d3812c3cbc69362b77dc0b19b345f8f5");
}

#[test]
#[allow(clippy::expect_used)]
fn geometry_msgs_transform_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("geometry_msgs", "Transform"))
        .expect("should compute hash");
    assert_eq!(result, "ac9eff44abf714214112b05d54a3cf9b");
}

#[test]
#[allow(clippy::expect_used)]
fn geometry_msgs_transform_stamped_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("geometry_msgs", "TransformStamped"))
        .expect("should compute hash");
    assert_eq!(result, "b5764a33bfeb3588febc2682852579b0");
}

#[test]
#[allow(clippy::expect_used)]
fn tf2_msgs_tfmessage_identity() {
    let specs = build_full_spec_set();
    let result = message_identity(&specs, &TypeName::new("tf2_msgs", "TFMessage"))
        .expect("should compute hash");
    assert_eq!(result, "94810edda583a504dfda3829e70d7eec");
}

#[test]
#[allow(clippy::expect_used)]
fn simple_type_no_separator() {
    let specs = build_full_spec_set();
    let text =
        definition_text(&specs, &TypeName::new("std_msgs", "Bool")).expect("should build text");
    assert_eq!(text, "bool data");
}

#[test]
#[allow(clippy::expect_used)]
fn pose_stamped_contains_dependencies_exactly_once() {
    let specs = build_full_spec_set();
    let text = definition_text(&specs, &TypeName::new("geometry_msgs", "PoseStamped"))
        .expect("should build text");

    let header_count = text.matches("MSG: std_msgs/Header").count();
    assert_eq!(header_count, 1, "Header should appear exactly once");

    let pose_count = text.matches("MSG: geometry_msgs/Pose").count();
    assert_eq!(pose_count, 1, "Pose should appear exactly once");

    let point_count = text.matches("MSG: geometry_msgs/Point").count();
    assert_eq!(point_count, 1, "Point should appear exactly once");

    let quaternion_count = text.matches("MSG: geometry_msgs/Quaternion").count();
    assert_eq!(quaternion_count, 1, "Quaternion should appear exactly once");
}

#[test]
#[allow(clippy::panic)]
fn missing_type_error() {
    let mut specs = SpecSet::new();
    specs.insert(MessageSpec {
        type_name: TypeName::new("test", "Root"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Message(TypeName::new("test", "Missing")),
            multiplicity: Multiplicity::Unit,
            name: "field".to_string(),
        }],
    });

    let result = message_identity(&specs, &TypeName::new("test", "Root"));
    match result {
        Err(ResolveError::MissingType { missing, referenced_by }) => {
            assert_eq!(missing.full(), "test/Missing");
            assert_eq!(referenced_by.full(), "test/Root");
        }
        _ => panic!("expected MissingType error"),
    }
}

#[test]
#[allow(clippy::panic)]
fn cycle_detection_direct() {
    let mut specs = SpecSet::new();
    specs.insert(MessageSpec {
        type_name: TypeName::new("test", "A"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Message(TypeName::new("test", "A")),
            multiplicity: Multiplicity::Unit,
            name: "self_ref".to_string(),
        }],
    });

    let result = message_identity(&specs, &TypeName::new("test", "A"));
    match result {
        Err(ResolveError::Cycle { at }) => {
            assert_eq!(at.full(), "test/A");
        }
        _ => panic!("expected Cycle error"),
    }
}

#[test]
#[allow(clippy::panic)]
fn cycle_detection_indirect() {
    let mut specs = SpecSet::new();
    specs.insert(MessageSpec {
        type_name: TypeName::new("test", "A"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Message(TypeName::new("test", "B")),
            multiplicity: Multiplicity::Unit,
            name: "field_b".to_string(),
        }],
    });
    specs.insert(MessageSpec {
        type_name: TypeName::new("test", "B"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Message(TypeName::new("test", "A")),
            multiplicity: Multiplicity::Unit,
            name: "field_a".to_string(),
        }],
    });

    let result = message_identity(&specs, &TypeName::new("test", "A"));
    match result {
        Err(ResolveError::Cycle { .. }) => {
            // Cycle detected as expected
        }
        _ => panic!("expected Cycle error"),
    }
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn constants_before_fields() {
    let mut specs = SpecSet::new();
    specs.insert(MessageSpec {
        type_name: TypeName::new("test", "WithConst"),
        constants: vec![
            Constant {
                constant_type: FieldType::Int32,
                name: "CONST_A".to_string(),
                value: "42".to_string(),
            },
            Constant {
                constant_type: FieldType::UInt32,
                name: "CONST_B".to_string(),
                value: "100".to_string(),
            },
        ],
        fields: vec![Field {
            field_type: FieldType::Float64,
            multiplicity: Multiplicity::Unit,
            name: "value".to_string(),
        }],
    });

    let text =
        definition_text(&specs, &TypeName::new("test", "WithConst")).expect("should build text");
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(lines[0], "int32 CONST_A=42");
    assert_eq!(lines[1], "uint32 CONST_B=100");
    assert_eq!(lines[2], "float64 value");
}

#[test]
#[allow(clippy::expect_used)]
fn fixed_array_field_rendering() {
    let mut specs = SpecSet::new();
    specs.insert(MessageSpec {
        type_name: TypeName::new("test", "ArrayTest"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::Float64,
            multiplicity: Multiplicity::Fixed(3),
            name: "covariance".to_string(),
        }],
    });

    let text =
        definition_text(&specs, &TypeName::new("test", "ArrayTest")).expect("should build text");
    assert_eq!(text, "float64[3] covariance");
}

#[test]
#[allow(clippy::expect_used)]
fn variable_array_field_rendering() {
    let mut specs = SpecSet::new();
    specs.insert(MessageSpec {
        type_name: TypeName::new("test", "VarArray"),
        constants: vec![],
        fields: vec![Field {
            field_type: FieldType::UInt8,
            multiplicity: Multiplicity::Variable,
            name: "data".to_string(),
        }],
    });

    let text =
        definition_text(&specs, &TypeName::new("test", "VarArray")).expect("should build text");
    assert_eq!(text, "uint8[] data");
}

#[test]
#[allow(clippy::expect_used, clippy::panic)]
fn missing_root_type() {
    let specs = SpecSet::new();
    let result = message_identity(&specs, &TypeName::new("test", "NonExistent"));
    match result {
        Err(ResolveError::MissingType { missing, referenced_by }) => {
            assert_eq!(missing.full(), "test/NonExistent");
            assert_eq!(referenced_by.full(), "test/NonExistent");
        }
        _ => panic!("expected MissingType error for root"),
    }
}

#[test]
#[allow(clippy::expect_used)]
fn add_two_ints_service_has_the_ros_identity() {
    let service = parse_service(
        TypeName::new("rospy_tutorials", "AddTwoInts"),
        "int64 a\nint64 b\n---\nint64 sum\n",
    )
    .expect("service parses");
    let mut specs = SpecSet::new();
    specs.insert(service.request.clone());
    specs.insert(service.response.clone());
    let identity = service_identity(&specs, &service).expect("identity resolves");
    assert_eq!(identity.service, "6a2e34150c00229791cc89ff309fff21");
}

#[test]
#[allow(clippy::expect_used)]
fn service_halves_carry_their_own_ros_identity() {
    // The request and response MD5 values are the ones ROS 1 gives AddTwoInts' halves.
    let service = parse_service(
        TypeName::new("rospy_tutorials", "AddTwoInts"),
        "int64 a\nint64 b\n---\nint64 sum\n",
    )
    .expect("service parses");
    let mut specs = SpecSet::new();
    specs.insert(service.request.clone());
    specs.insert(service.response.clone());
    let identity = service_identity(&specs, &service).expect("identity resolves");
    assert_eq!(identity.service, "6a2e34150c00229791cc89ff309fff21");
    assert_eq!(identity.request, "36d09b846be0b371c5f190354dd3153e");
    assert_eq!(identity.response, "b88405221c77b1878a3cbbfff53428d7");
}

/// A chain of `levels` message types, `p/T0` holding `p/T1` holding … holding an `int32`.
#[allow(clippy::expect_used)]
fn chain_specs(levels: usize) -> SpecSet {
    let mut specs = SpecSet::new();
    for level in 0..levels {
        let next = level.saturating_add(1);
        let body =
            if next < levels { format!("p/T{next} child") } else { String::from("int32 value") };
        let spec = parse_message(TypeName::new("p", &format!("T{level}")), &body)
            .expect("chain body parses");
        specs.insert(spec);
    }
    specs
}

#[test]
#[allow(clippy::expect_used, clippy::panic)]
fn a_chain_at_the_nesting_limit_resolves_and_one_level_more_is_refused() {
    let root = TypeName::new("p", "T0");
    message_identity(&chain_specs(MAX_NESTING_DEPTH), &root)
        .expect("a chain of MAX_NESTING_DEPTH types resolves");
    let result = message_identity(&chain_specs(MAX_NESTING_DEPTH + 1), &root);
    let expected_at = TypeName::new("p", &format!("T{MAX_NESTING_DEPTH}"));
    assert_eq!(
        result,
        Err(ResolveError::NestingTooDeep { limit: MAX_NESTING_DEPTH, at: expected_at })
    );
}

/// `p/Root` holds `p/T{j}` for every multiple of 60 below `levels`, descending, so each
/// shared type is first reached on a shallow path; the chain from `p/T0` holds an `int32`.
#[allow(clippy::expect_used)]
fn shared_chain_specs(levels: usize) -> SpecSet {
    let mut specs = SpecSet::new();
    let shared: Vec<usize> = (0..levels).step_by(60).rev().collect();
    let mut root_body = String::new();
    for index in shared {
        let _ = writeln!(root_body, "p/T{index} f{index}");
    }
    specs.insert(parse_message(TypeName::new("p", "Root"), &root_body).expect("root body parses"));
    for level in 0..levels {
        let next = level.saturating_add(1);
        let body =
            if next < levels { format!("p/T{next} child") } else { String::from("int32 value") };
        let spec = parse_message(TypeName::new("p", &format!("T{level}")), &body)
            .expect("chain body parses");
        specs.insert(spec);
    }
    specs
}

#[test]
#[allow(clippy::expect_used, clippy::panic)]
fn a_type_first_reached_on_a_shallow_path_has_its_deep_path_checked() {
    let root = TypeName::new("p", "Root");
    let result = message_identity(&shared_chain_specs(200), &root);
    assert!(
        matches!(result, Err(ResolveError::NestingTooDeep { limit: MAX_NESTING_DEPTH, .. })),
        "a 201-level chain resolved: {result:?}"
    );
}

#[test]
#[cfg_attr(miri, ignore = "a scale test: the chain takes Miri more than ten minutes")]
#[allow(clippy::expect_used, clippy::panic)]
fn a_large_shared_chain_is_refused_at_the_limit_without_recursing() {
    let root = TypeName::new("p", "Root");
    let result = message_identity(&shared_chain_specs(200_000), &root);
    assert!(
        matches!(result, Err(ResolveError::NestingTooDeep { limit: MAX_NESTING_DEPTH, .. })),
        "a 200000-level chain resolved: {result:?}"
    );
}

#[test]
#[allow(clippy::expect_used)]
fn a_definition_that_expands_past_the_cap_is_refused() {
    let mut specs = SpecSet::new();
    for level in 0..17_usize {
        let text = if level == 16 {
            String::from("int8 x\n")
        } else {
            format!("p/T{} a\np/T{} b\n", level + 1, level + 1)
        };
        specs.insert(
            parse_message(TypeName::new("p", &format!("T{level}")), &text).expect("parses"),
        );
    }
    let root = TypeName::new("p", "T0");
    let refused = hypha_msgdef::resolve::check_expansion(&specs, &root);
    let expected = ResolveError::TooLarge { limit: 65_536, at: root.clone() };
    assert_eq!(refused, Err(expected.clone()));
    assert_eq!(expected.to_string(), "type `p/T0` expands to more than 65536 nested messages");
    assert!(message_identity(&specs, &root).is_err());
}

#[test]
#[allow(clippy::expect_used)]
fn a_fixed_array_of_messages_counts_its_length() {
    let element = parse_message(TypeName::new("p", "E"), "int8 x\n").expect("parses");
    let root = TypeName::new("p", "Holder");
    for (length, accepted) in [(100_usize, true), (65_536, false)] {
        let mut specs = SpecSet::new();
        specs.insert(element.clone());
        specs.insert(parse_message(root.clone(), &format!("p/E[{length}] e\n")).expect("parses"));
        let checked = hypha_msgdef::resolve::check_expansion(&specs, &root);
        assert_eq!(checked.is_ok(), accepted, "length {length}: {checked:?}");
    }
}
