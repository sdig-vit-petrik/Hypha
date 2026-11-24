//! Integration tests for `hypha_msgdef::parse`.

use hypha_msgdef::definition::parse_definition;
use hypha_msgdef::parse::{ParseError, parse_message};
use hypha_msgdef::spec::{Constant, Field, FieldType, MessageSpec, Multiplicity, TypeName};

fn own_type() -> TypeName {
    TypeName::new("my_pkg", "MyMsg")
}

fn field(field_type: FieldType, multiplicity: Multiplicity, name: &str) -> Field {
    Field { field_type, multiplicity, name: String::from(name) }
}

fn constant(constant_type: FieldType, name: &str, value: &str) -> Constant {
    Constant { constant_type, name: String::from(name), value: String::from(value) }
}

fn spec_with_fields(fields: Vec<Field>) -> MessageSpec {
    MessageSpec { type_name: own_type(), constants: Vec::new(), fields }
}

#[test]
fn simple_message_has_three_fields_in_order() {
    let text = "uint32 seq\ntime stamp\nstring frame_id\n";

    let expected = spec_with_fields(vec![
        field(FieldType::UInt32, Multiplicity::Unit, "seq"),
        field(FieldType::Time, Multiplicity::Unit, "stamp"),
        field(FieldType::Text, Multiplicity::Unit, "frame_id"),
    ]);

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn every_builtin_name_maps_to_its_variant() {
    let cases = [
        ("bool", FieldType::Bool),
        ("byte", FieldType::Byte),
        ("char", FieldType::Char),
        ("int8", FieldType::Int8),
        ("uint8", FieldType::UInt8),
        ("int16", FieldType::Int16),
        ("uint16", FieldType::UInt16),
        ("int32", FieldType::Int32),
        ("uint32", FieldType::UInt32),
        ("int64", FieldType::Int64),
        ("uint64", FieldType::UInt64),
        ("float32", FieldType::Float32),
        ("float64", FieldType::Float64),
        ("string", FieldType::Text),
        ("time", FieldType::Time),
        ("duration", FieldType::Duration),
    ];

    for (name, builtin_type) in cases {
        let text = format!("{name} value");
        let expected = spec_with_fields(vec![field(builtin_type, Multiplicity::Unit, "value")]);
        assert_eq!(parse_message(own_type(), &text), Ok(expected));
    }
}

#[test]
fn header_resolves_to_std_msgs_header() {
    let expected_type = FieldType::Message(TypeName::new("std_msgs", "Header"));
    let expected = spec_with_fields(vec![field(expected_type, Multiplicity::Unit, "header")]);

    assert_eq!(parse_message(own_type(), "Header header"), Ok(expected));
}

#[test]
fn unqualified_nested_type_takes_the_parsed_message_package() {
    let expected_type = FieldType::Message(TypeName::new("my_pkg", "Odometry"));
    let expected = spec_with_fields(vec![field(expected_type, Multiplicity::Unit, "odom")]);

    assert_eq!(parse_message(own_type(), "Odometry odom"), Ok(expected));
}

#[test]
fn qualified_nested_type_keeps_its_own_package() {
    let expected_type = FieldType::Message(TypeName::new("geometry_msgs", "Point"));
    let expected = spec_with_fields(vec![field(expected_type, Multiplicity::Unit, "position")]);

    let text = "geometry_msgs/Point position";
    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn array_suffixes_give_the_right_multiplicity() {
    let text = "float64[3] covariance\ngeometry_msgs/Point[] points\nuint32 seq\n";
    let point_type = FieldType::Message(TypeName::new("geometry_msgs", "Point"));

    let expected = spec_with_fields(vec![
        field(FieldType::Float64, Multiplicity::Fixed(3), "covariance"),
        field(point_type, Multiplicity::Variable, "points"),
        field(FieldType::UInt32, Multiplicity::Unit, "seq"),
    ]);

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let text = "# a whole comment line\n\nuint32 seq\n   \n# another\nstring frame_id\n";

    let expected = spec_with_fields(vec![
        field(FieldType::UInt32, Multiplicity::Unit, "seq"),
        field(FieldType::Text, Multiplicity::Unit, "frame_id"),
    ]);

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn trailing_comment_on_a_field_line_is_stripped() {
    let text = "uint32 seq # a running counter";
    let expected = spec_with_fields(vec![field(FieldType::UInt32, Multiplicity::Unit, "seq")]);

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn crlf_line_endings_work() {
    let text = "uint32 seq\r\nstring frame_id\r\n";

    let expected = spec_with_fields(vec![
        field(FieldType::UInt32, Multiplicity::Unit, "seq"),
        field(FieldType::Text, Multiplicity::Unit, "frame_id"),
    ]);

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn string_constant_keeps_hash_and_spaces_verbatim() {
    let text = "string A=hello # not a comment";
    let expected = MessageSpec {
        type_name: own_type(),
        constants: vec![constant(FieldType::Text, "A", "hello # not a comment")],
        fields: Vec::new(),
    };

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn numeric_constant_is_trimmed_and_comment_stripped() {
    let text = "int32 FOO=  5  # a magic number";
    let expected = MessageSpec {
        type_name: own_type(),
        constants: vec![constant(FieldType::Int32, "FOO", "5")],
        fields: Vec::new(),
    };

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn string_equation_splits_on_the_first_equals_only() {
    let text = "string EQUATION=a=b";
    let expected = MessageSpec {
        type_name: own_type(),
        constants: vec![constant(FieldType::Text, "EQUATION", "a=b")],
        fields: Vec::new(),
    };

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn constants_and_fields_land_in_separate_vectors_in_written_order() {
    let text = "int32 FIRST=1\nuint32 seq\nint32 SECOND=2\nstring frame_id\n";
    let expected = MessageSpec {
        type_name: own_type(),
        constants: vec![
            constant(FieldType::Int32, "FIRST", "1"),
            constant(FieldType::Int32, "SECOND", "2"),
        ],
        fields: vec![
            field(FieldType::UInt32, Multiplicity::Unit, "seq"),
            field(FieldType::Text, Multiplicity::Unit, "frame_id"),
        ],
    };

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn line_with_type_but_no_name_is_an_error() {
    let expected = Err(ParseError::MissingName { line: 1, text: String::from("uint32") });

    assert_eq!(parse_message(own_type(), "uint32"), expected);
}

#[test]
fn empty_type_from_a_bare_array_suffix_is_an_error() {
    let text = "uint32 seq\n[3] bad\n";
    let expected = Err(ParseError::InvalidType { line: 2, text: String::from("[3]") });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn array_size_with_letters_is_an_error() {
    let text = "uint32 seq\nint32[abc] bad\n";
    let expected = Err(ParseError::InvalidArraySize { line: 2, text: String::from("int32[abc]") });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn unclosed_array_bracket_is_an_error() {
    let text = "uint32 seq\nint32[3 bad\n";
    let expected = Err(ParseError::InvalidArraySize { line: 2, text: String::from("int32[3") });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn empty_field_name_is_an_error() {
    let text = "uint32 seq\nstring # just a comment\n";
    let expected = Err(ParseError::EmptyName { line: 2 });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn empty_constant_name_is_an_error() {
    let text = "uint32 seq\nint32 =5\n";
    let expected = Err(ParseError::EmptyName { line: 2 });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn a_ros2_default_value_after_a_field_name_is_refused() {
    let result = parse_message(own_type(), "uint32 seq\nfloat64 x 1.0\n");
    let expected =
        ParseError::TrailingText { line: 2, name: String::from("x"), text: String::from("1.0") };

    assert_eq!(result, Err(expected.clone()));
    assert_eq!(
        expected.to_string(),
        "line 2: unexpected `1.0` after field `x`; a default value is ROS 2 syntax, which is not supported"
    );
}

#[test]
fn a_second_word_after_a_field_name_is_refused() {
    let expected =
        ParseError::TrailingText { line: 1, name: String::from("x"), text: String::from("y") };

    assert_eq!(parse_message(own_type(), "int32 x y # comment\n"), Err(expected));
}

#[test]
fn a_field_name_that_is_not_an_identifier_is_refused() {
    for name in ["a-b", "2x", "_x", "x.y", "é"] {
        let text = format!("uint32 seq\nint32 {name}\n");
        let expected = ParseError::InvalidName { line: 2, text: String::from(name) };
        assert_eq!(parse_message(own_type(), &text), Err(expected), "name {name}");
    }
    assert_eq!(
        ParseError::InvalidName { line: 2, text: String::from("a-b") }.to_string(),
        "line 2: field or constant name `a-b` is not a letter followed by letters, digits or `_`"
    );
}

#[test]
fn a_constant_name_that_is_not_an_identifier_is_refused() {
    for name in ["2X", "A-B", "A B"] {
        let text = format!("int32 {name}=1\n");
        let expected = ParseError::InvalidName { line: 1, text: String::from(name) };
        assert_eq!(parse_message(own_type(), &text), Err(expected), "name {name}");
    }
}

#[test]
fn names_with_digits_and_underscores_are_accepted() {
    let text = "int32 MAX_2 = 7\nfloat64 x\nint32 point2d_a__b_\nuint8[3] Rgb_8\nstring class\n";
    let expected = MessageSpec {
        type_name: own_type(),
        constants: vec![constant(FieldType::Int32, "MAX_2", "7")],
        fields: vec![
            field(FieldType::Float64, Multiplicity::Unit, "x"),
            field(FieldType::Int32, Multiplicity::Unit, "point2d_a__b_"),
            field(FieldType::UInt8, Multiplicity::Fixed(3), "Rgb_8"),
            field(FieldType::Text, Multiplicity::Unit, "class"),
        ],
    };

    assert_eq!(parse_message(own_type(), text), Ok(expected));
}

#[test]
fn a_definition_from_the_bus_with_a_bad_field_name_is_refused() {
    let text = "uint32 seq\n\
                other_pkg/Inner inner\n\
                ================================================================================\n\
                MSG: other_pkg/Inner\n\
                float64 x 1.0\n";
    let error = parse_definition(&own_type(), text).map(|_| ()).map_err(|error| error.to_string());

    assert_eq!(
        error,
        Err(String::from(
            "section `other_pkg/Inner` is not a valid message: line 1: unexpected `1.0` after \
             field `x`; a default value is ROS 2 syntax, which is not supported"
        ))
    );
}

#[test]
fn type_name_with_empty_package_is_an_error() {
    let text = "uint32 seq\n/Foo bad\n";
    let expected = Err(ParseError::InvalidTypeName { line: 2, text: String::from("/Foo") });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn type_name_with_empty_name_is_an_error() {
    let text = "uint32 seq\nfoo/ bad\n";
    let expected = Err(ParseError::InvalidTypeName { line: 2, text: String::from("foo/") });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn nested_message_constant_is_an_error() {
    let text = "uint32 seq\ngeometry_msgs/Point BAD=1\n";
    let expected =
        Err(ParseError::NonBuiltinConstant { line: 2, text: String::from("geometry_msgs/Point") });

    assert_eq!(parse_message(own_type(), text), expected);
}

#[test]
fn render_type_round_trips_a_fixed_array_builtin() {
    let expected =
        spec_with_fields(vec![field(FieldType::Float64, Multiplicity::Fixed(3), "covariance")]);
    assert_eq!(parse_message(own_type(), "float64[3] covariance"), Ok(expected.clone()));

    let rendered_type = expected.fields.first().map(Field::render_type);
    assert_eq!(rendered_type, Some(String::from("float64[3]")));
}

#[test]
fn render_type_round_trips_a_variable_array_and_a_plain_type() {
    let text = "uint32[] counts\nuint32 seq\n";
    let expected = spec_with_fields(vec![
        field(FieldType::UInt32, Multiplicity::Variable, "counts"),
        field(FieldType::UInt32, Multiplicity::Unit, "seq"),
    ]);
    assert_eq!(parse_message(own_type(), text), Ok(expected.clone()));

    let variable_rendered = expected.fields.first().map(Field::render_type);
    let plain_rendered = expected.fields.get(1).map(Field::render_type);
    assert_eq!(variable_rendered, Some(String::from("uint32[]")));
    assert_eq!(plain_rendered, Some(String::from("uint32")));
}

fn parse_one_constant(declaration: &str) -> Result<MessageSpec, ParseError> {
    parse_message(own_type(), declaration)
}

#[test]
fn a_byte_constant_of_200_is_out_of_range() {
    let result = parse_one_constant("byte X=200\n");
    let expected = ParseError::ConstantOutOfRange {
        line: 1,
        text: String::from("200"),
        constant_type: String::from("byte"),
        min: String::from("-128"),
        max: String::from("127"),
    };
    assert_eq!(result, Err(expected.clone()));
    assert_eq!(
        expected.to_string(),
        "line 1: constant value 200 is out of range for byte (-128 to 127)"
    );

    let char_result = parse_one_constant("char X=-1\n");
    assert!(matches!(char_result, Err(ParseError::ConstantOutOfRange { .. })));
}

#[test]
fn each_integer_type_accepts_both_limits_and_refuses_one_past() {
    let types: [(&str, i128, i128); 10] = [
        ("byte", -128, 127),
        ("char", 0, 255),
        ("int8", -128, 127),
        ("uint8", 0, 255),
        ("int16", i128::from(i16::MIN), i128::from(i16::MAX)),
        ("uint16", 0, i128::from(u16::MAX)),
        ("int32", i128::from(i32::MIN), i128::from(i32::MAX)),
        ("uint32", 0, i128::from(u32::MAX)),
        ("int64", i128::from(i64::MIN), i128::from(i64::MAX)),
        ("uint64", 0, i128::from(u64::MAX)),
    ];
    for (type_text, min, max) in types {
        for accepted in [min, max] {
            let text = format!("{type_text} X={accepted}\n");
            assert!(parse_one_constant(&text).is_ok(), "{text} is refused");
        }
        let below = min.checked_sub(1).unwrap_or(min);
        let above = max.checked_add(1).unwrap_or(max);
        for refused in [below, above] {
            let text = format!("{type_text} X={refused}\n");
            let result = parse_one_constant(&text);
            let out_of_range = matches!(result, Err(ParseError::ConstantOutOfRange { .. }));
            assert!(out_of_range, "{text} gives {result:?}");
        }
    }
}

#[test]
fn a_hexadecimal_integer_constant_is_accepted_and_kept_as_written() {
    let text = "uint32 MASK=0x01000000\nint8 LOW=-0x80\n";
    let expected = MessageSpec {
        type_name: own_type(),
        constants: vec![
            constant(FieldType::UInt32, "MASK", "0x01000000"),
            constant(FieldType::Int8, "LOW", "-0x80"),
        ],
        fields: Vec::new(),
    };
    assert_eq!(parse_message(own_type(), text), Ok(expected));

    let too_wide = parse_one_constant("uint8 X=0x100\n");
    assert!(matches!(too_wide, Err(ParseError::ConstantOutOfRange { .. })));
}

#[test]
fn a_non_literal_integer_constant_is_refused() {
    for value in ["1_000", "1.0", "", "0x", "0xG1", "ten", "+-1"] {
        let text = format!("int32 X={value}\n");
        let expected = ParseError::InvalidConstantValue {
            line: 1,
            text: String::from(value),
            constant_type: String::from("int32"),
        };
        assert_eq!(parse_one_constant(&text), Err(expected), "{text}");
    }
    let shown = ParseError::InvalidConstantValue {
        line: 3,
        text: String::from("ten"),
        constant_type: String::from("int32"),
    };
    assert_eq!(shown.to_string(), "line 3: constant value \"ten\" is not a int32 literal");
}

#[test]
fn an_integer_literal_longer_than_any_type_is_out_of_range() {
    let result = parse_one_constant("uint64 X=999999999999999999999999999999999999999999\n");
    assert!(matches!(result, Err(ParseError::ConstantOutOfRange { .. })));
}

#[test]
fn a_float_constant_must_be_a_finite_literal() {
    for value in ["-1", "1e3", "1.5", ".5", "2.", "-1.25E-3"] {
        let text = format!("float64 X={value}\n");
        assert!(parse_one_constant(&text).is_ok(), "{text} is refused");
    }
    for value in ["inf", "nan", "abc", "1e", "0x10", "."] {
        let text = format!("float64 X={value}\n");
        let result = parse_one_constant(&text);
        let refused = matches!(result, Err(ParseError::InvalidConstantValue { .. }));
        assert!(refused, "{text} gives {result:?}");
    }
    let huge = parse_one_constant("float64 X=1e400\n");
    assert!(matches!(huge, Err(ParseError::ConstantOutOfRange { .. })));
    let past_float32 = parse_one_constant("float32 X=1e39\n");
    assert!(matches!(past_float32, Err(ParseError::ConstantOutOfRange { .. })));
}

#[test]
fn a_bool_constant_takes_six_spellings() {
    for value in ["true", "false", "True", "False", "1", "0"] {
        let text = format!("bool X={value}\n");
        assert!(parse_one_constant(&text).is_ok(), "{text} is refused");
    }
    for value in ["TRUE", "yes", "2", ""] {
        let text = format!("bool X={value}\n");
        let result = parse_one_constant(&text);
        let refused = matches!(result, Err(ParseError::InvalidConstantValue { .. }));
        assert!(refused, "{text} gives {result:?}");
    }
}

#[test]
fn a_time_or_duration_constant_is_refused() {
    for type_text in ["time", "duration"] {
        let text = format!("{type_text} X=1\n");
        let expected = ParseError::NonBuiltinConstant { line: 1, text: String::from(type_text) };
        assert_eq!(parse_one_constant(&text), Err(expected));
    }
}

#[test]
fn an_array_constant_is_refused() {
    for type_text in ["int32[]", "int32[2]"] {
        let text = format!("{type_text} X=1\n");
        let expected = ParseError::ArrayConstant { line: 1, text: String::from(type_text) };
        assert_eq!(parse_one_constant(&text), Err(expected));
    }
}

#[test]
fn a_constant_value_keeps_its_comment_out() {
    let expected = MessageSpec {
        type_name: own_type(),
        constants: vec![constant(FieldType::Int32, "X", "5")],
        fields: Vec::new(),
    };
    assert_eq!(parse_one_constant("int32 X=5 # five\n"), Ok(expected));
}

#[test]
fn every_message_in_the_tree_parses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut pending = vec![root.join("crates"), root.join("examples"), root.join("apps")];
    let mut parsed = 0_usize;
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let skipped = name == "target" || name == "build" || name == "fixtures";
            if path.is_dir() && !skipped {
                pending.push(path);
                continue;
            }
            let is_message = path.extension().is_some_and(|extension| extension == "msg");
            if !is_message {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let result = parse_message(own_type(), &text);
            assert!(result.is_ok(), "{}: {result:?}", path.display());
            parsed = parsed.saturating_add(1);
        }
    }
    assert!(parsed > 50, "only {parsed} message files found");
}
