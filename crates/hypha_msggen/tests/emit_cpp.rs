//! Tests for C++ message source generation.

use hypha_msgdef::resolve::SpecSet;
use hypha_msgdef::spec::{Constant, Field, FieldType, MessageSpec, Multiplicity, TypeName};
use hypha_msggen::emit_cpp::emit_message;

fn field(field_type: FieldType, multiplicity: Multiplicity, name: &str) -> Field {
    Field { field_type, multiplicity, name: String::from(name) }
}

fn constant(constant_type: FieldType, name: &str, value: &str) -> Constant {
    Constant { constant_type, name: String::from(name), value: String::from(value) }
}

fn spec(type_name: TypeName, constants: Vec<Constant>, fields: Vec<Field>) -> MessageSpec {
    MessageSpec { type_name, constants, fields }
}

fn emit(spec: &MessageSpec, md5: &str) -> String {
    emit_with(spec, Vec::new(), md5)
}

/// Emits `spec` with the nested types in `dependencies` defined in the set.
#[allow(clippy::expect_used)]
fn emit_with(spec: &MessageSpec, dependencies: Vec<MessageSpec>, md5: &str) -> String {
    let mut specs = SpecSet::new();
    specs.insert(spec.clone());
    for dependency in dependencies {
        specs.insert(dependency);
    }
    emit_message(&specs, spec, md5).expect("message emits")
}

/// A nested type with one field, enough for the message to resolve.
fn stub(package: &str, name: &str) -> MessageSpec {
    spec(
        TypeName::new(package, name),
        Vec::new(),
        vec![field(FieldType::Float64, Multiplicity::Unit, "x")],
    )
}

#[test]
// This test compares a complete generated header to its golden text.
#[allow(clippy::too_many_lines)]
fn std_msgs_header_exact_output() {
    let type_name = TypeName::new("std_msgs", "Header");
    let fields = vec![
        field(FieldType::UInt32, Multiplicity::Unit, "seq"),
        field(FieldType::Time, Multiplicity::Unit, "stamp"),
        field(FieldType::Text, Multiplicity::Unit, "frame_id"),
    ];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let expected = concat!(
        "// Generated from the std_msgs/Header definition. Do not edit.\n",
        "\n",
        "#pragma once\n",
        "\n",
        "#include <array>\n",
        "#include <cstdint>\n",
        "#include <memory>\n",
        "#include <string>\n",
        "#include <type_traits>\n",
        "#include <vector>\n",
        "\n",
        "#include <ros/message_traits.h>\n",
        "#include <ros/serialization.h>\n",
        "#include <hypha_msgs/ros_time.h>\n",
        "\n",
        "namespace std_msgs {\n",
        "\n",
        "struct Header {\n",
        "  uint32_t seq{};\n",
        "  hypha_msgs::RosTime stamp{};\n",
        "  std::string frame_id{};\n",
        "\n",
        "  using _seq_type = uint32_t;\n",
        "  using _stamp_type = hypha_msgs::RosTime;\n",
        "  using _frame_id_type = std::string;\n",
        "  Header& set__seq(const _seq_type& _arg) {\n",
        "    this->seq = _arg;\n",
        "    return *this;\n",
        "  }\n",
        "  Header& set__stamp(const _stamp_type& _arg) {\n",
        "    this->stamp = _arg;\n",
        "    return *this;\n",
        "  }\n",
        "  Header& set__frame_id(const _frame_id_type& _arg) {\n",
        "    this->frame_id = _arg;\n",
        "    return *this;\n",
        "  }\n",
        "\n",
        "  bool operator==(const Header&) const = default;\n",
        "\n",
        "  using Ptr = std::shared_ptr<Header>;\n",
        "  using ConstPtr = std::shared_ptr<const Header>;\n",
        "  using SharedPtr = std::shared_ptr<Header>;\n",
        "  using ConstSharedPtr = std::shared_ptr<const Header>;\n",
        "  using UniquePtr = std::unique_ptr<Header>;\n",
        "\n",
        "  static constexpr const char* TYPE_NAME = \"std_msgs/Header\";\n",
        "  static constexpr const char* MD5 = \"2176decaecbce78abc3b96ef049fabed\";\n",
        "};\n",
        "\n",
        "using HeaderPtr = Header::Ptr;\n",
        "using HeaderConstPtr = Header::ConstPtr;\n",
        "\n",
        "}  // namespace std_msgs\n",
        "\n",
        "namespace ros::message_traits {\n",
        "\n",
        "template <>\n",
        "struct IsMessage<::std_msgs::Header> : std::true_type {};\n",
        "\n",
        "template <>\n",
        "struct IsMessage<const ::std_msgs::Header> : std::true_type {};\n",
        "\n",
        "template <>\n",
        "struct IsFixedSize<::std_msgs::Header> : std::bool_constant<false> {};\n",
        "\n",
        "template <>\n",
        "struct IsFixedSize<const ::std_msgs::Header> : std::bool_constant<false> {};\n",
        "\n",
        "template <>\n",
        "struct HasHeader<::std_msgs::Header> : std::bool_constant<false> {};\n",
        "\n",
        "template <>\n",
        "struct HasHeader<const ::std_msgs::Header> : std::bool_constant<false> {};\n",
        "\n",
        "template <>\n",
        "struct DataType<::std_msgs::Header> {\n",
        "  static const char* value() {\n",
        "    return \"std_msgs/Header\";\n",
        "  }\n",
        "  static const char* value(const ::std_msgs::Header&) {\n",
        "    return value();\n",
        "  }\n",
        "};\n",
        "\n",
        "template <>\n",
        "struct MD5Sum<::std_msgs::Header> {\n",
        "  static const char* value() {\n",
        "    return \"2176decaecbce78abc3b96ef049fabed\";\n",
        "  }\n",
        "  static const char* value(const ::std_msgs::Header&) {\n",
        "    return value();\n",
        "  }\n",
        "};\n",
        "\n",
        "template <>\n",
        "struct Definition<::std_msgs::Header> {\n",
        "  static const char* value() {\n",
        "    return \"uint32 seq\\n\"\n",
        "    \"time stamp\\n\"\n",
        "    \"string frame_id\";\n",
        "  }\n",
        "  static const char* value(const ::std_msgs::Header&) {\n",
        "    return value();\n",
        "  }\n",
        "};\n",
        "\n",
        "}  // namespace ros::message_traits\n",
        "\n",
        "namespace ros::serialization {\n",
        "\n",
        "template <>\n",
        "struct Serializer<::std_msgs::Header> {\n",
        "  template <typename Stream, typename T>\n",
        "  inline static void allInOne(Stream& stream, T m) {\n",
        "    stream.next(m.seq);\n",
        "    stream.next(m.stamp);\n",
        "    stream.next(m.frame_id);\n",
        "  }\n",
        "\n",
        "  ROS_DECLARE_ALLINONE_SERIALIZER\n",
        "};\n",
        "\n",
        "}  // namespace ros::serialization\n",
    );

    assert_eq!(emit(&msg_spec, "2176decaecbce78abc3b96ef049fabed"), expected);
}

#[test]
fn single_float64_field() {
    let type_name = TypeName::new("sensor_msgs", "Data");
    let fields = vec![field(FieldType::Float64, Multiplicity::Unit, "data")];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit(&msg_spec, "abc123");

    assert!(result.contains("struct Data {"));
    assert!(result.contains("  double data{};"));
    assert!(result.contains("namespace sensor_msgs {"));
}

#[test]
fn nested_message_type_with_include() {
    let type_name = TypeName::new("geometry_msgs", "PoseStamped");
    let fields = vec![field(
        FieldType::Message(TypeName::new("geometry_msgs", "Point")),
        Multiplicity::Unit,
        "position",
    )];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit_with(&msg_spec, vec![stub("geometry_msgs", "Point")], "def456");

    assert!(result.contains("#include <hypha_msgs/geometry_msgs/Point.h>"));
    assert!(result.contains("geometry_msgs::Point position{};"));
}

#[test]
fn two_fields_same_nested_type_one_include() {
    let type_name = TypeName::new("nav_msgs", "Path");
    let fields = vec![
        field(
            FieldType::Message(TypeName::new("geometry_msgs", "Point")),
            Multiplicity::Unit,
            "start",
        ),
        field(
            FieldType::Message(TypeName::new("geometry_msgs", "Point")),
            Multiplicity::Unit,
            "end",
        ),
    ];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit_with(&msg_spec, vec![stub("geometry_msgs", "Point")], "xyz789");

    let include_count = result.matches("#include <hypha_msgs/geometry_msgs/Point.h>").count();
    assert_eq!(include_count, 1, "Include should appear exactly once");
    assert!(result.contains("geometry_msgs::Point start{};"));
    assert!(result.contains("geometry_msgs::Point end{};"));
}

#[test]
fn variable_array_of_nested_type() {
    let type_name = TypeName::new("nav_msgs", "Path");
    let fields = vec![field(
        FieldType::Message(TypeName::new("geometry_msgs", "Point")),
        Multiplicity::Variable,
        "points",
    )];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit_with(&msg_spec, vec![stub("geometry_msgs", "Point")], "arr123");

    assert!(result.contains("#include <hypha_msgs/geometry_msgs/Point.h>"));
    assert!(result.contains("std::vector<geometry_msgs::Point> points{};"));
}

/// A message of fixed-size nested messages is fixed-size: its trait defers to theirs.
#[test]
fn nested_fixed_messages_keep_the_message_fixed_size() {
    let type_name = TypeName::new("geometry_msgs", "Pose");
    let fields = vec![
        field(
            FieldType::Message(TypeName::new("geometry_msgs", "Point")),
            Multiplicity::Unit,
            "position",
        ),
        field(
            FieldType::Message(TypeName::new("geometry_msgs", "Quaternion")),
            Multiplicity::Fixed(2),
            "orientation",
        ),
    ];
    let msg_spec = spec(type_name, Vec::new(), fields);
    let dependencies = vec![stub("geometry_msgs", "Point"), stub("geometry_msgs", "Quaternion")];

    let result = emit_with(&msg_spec, dependencies, "pose123");

    assert!(result.contains(
        "struct IsFixedSize<::geometry_msgs::Pose> : std::bool_constant<\
         IsFixedSize<::geometry_msgs::Point>::value && \
         IsFixedSize<::geometry_msgs::Quaternion>::value> {};"
    ));
}

/// A string or a variable array makes a message variable-size whatever its nested types.
#[test]
fn a_variable_field_makes_the_message_variable_size() {
    let fields = vec![
        field(
            FieldType::Message(TypeName::new("geometry_msgs", "Point")),
            Multiplicity::Unit,
            "start",
        ),
        field(FieldType::Float64, Multiplicity::Variable, "weights"),
    ];
    let msg_spec = spec(TypeName::new("nav_msgs", "Weights"), Vec::new(), fields);

    let result = emit_with(&msg_spec, vec![stub("geometry_msgs", "Point")], "var123");

    assert!(
        result.contains("struct IsFixedSize<::nav_msgs::Weights> : std::bool_constant<false> {};")
    );
}

#[test]
fn single_primitive_message_is_fixed_size() {
    let fields = vec![field(FieldType::Time, Multiplicity::Unit, "clock")];
    let msg_spec = spec(TypeName::new("rosgraph_msgs", "Clock"), Vec::new(), fields);

    let result = emit(&msg_spec, "clock123");

    assert!(
        result
            .contains("struct IsFixedSize<::rosgraph_msgs::Clock> : std::bool_constant<true> {};")
    );
}

#[test]
fn fixed_array_double() {
    let type_name = TypeName::new("sensor_msgs", "CovarianceMatrix");
    let fields = vec![field(FieldType::Float64, Multiplicity::Fixed(3), "covariance")];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit(&msg_spec, "cov123");

    assert!(result.contains("std::array<double, 3> covariance{};"));
}

#[test]
fn constants_integer_and_string() {
    let type_name = TypeName::new("my_msgs", "Config");
    let constants = vec![
        constant(FieldType::Int32, "TIMEOUT", "5"),
        constant(FieldType::Text, "MODE", "debug"),
    ];
    let fields = vec![field(FieldType::Bool, Multiplicity::Unit, "active")];
    let msg_spec = spec(type_name, constants, fields);

    let result = emit(&msg_spec, "cfg123");

    assert!(result.contains("  static constexpr int32_t TIMEOUT = 5;"));
    assert!(result.contains("  static constexpr const char* MODE = \"debug\";"));
}

#[test]
fn string_constant_with_escapes() {
    let type_name = TypeName::new("my_msgs", "Escaped");
    let constants = vec![constant(FieldType::Text, "PATH", "C:\\Users\\test\\file.txt")];
    let fields = vec![field(FieldType::Bool, Multiplicity::Unit, "flag")];
    let msg_spec = spec(type_name, constants, fields);

    let result = emit(&msg_spec, "esc123");

    assert!(
        result
            .contains("  static constexpr const char* PATH = \"C:\\\\Users\\\\test\\\\file.txt\";")
    );
}

#[test]
fn string_constant_with_quotes() {
    let type_name = TypeName::new("my_msgs", "Quoted");
    let constants = vec![constant(FieldType::Text, "LABEL", "He said \"hello\"")];
    let fields = vec![field(FieldType::Bool, Multiplicity::Unit, "flag")];
    let msg_spec = spec(type_name, constants, fields);

    let result = emit(&msg_spec, "quo123");

    assert!(result.contains("  static constexpr const char* LABEL = \"He said \\\"hello\\\"\";"));
}

#[test]
fn field_named_class_becomes_class_with_underscore() {
    let type_name = TypeName::new("my_msgs", "Data");
    let fields = vec![field(FieldType::Int32, Multiplicity::Unit, "class")];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit(&msg_spec, "kw123");

    assert!(result.contains("  int32_t class_{};"));
}

#[test]
fn duration_field_type() {
    let type_name = TypeName::new("my_msgs", "Timing");
    let fields = vec![field(FieldType::Duration, Multiplicity::Unit, "elapsed")];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit(&msg_spec, "dur123");

    assert!(result.contains("  hypha_msgs::RosDuration elapsed{};"));
}

#[test]
fn all_builtin_types() {
    let type_name = TypeName::new("test_msgs", "AllTypes");
    let fields = vec![
        field(FieldType::Bool, Multiplicity::Unit, "bool_val"),
        field(FieldType::Byte, Multiplicity::Unit, "byte_val"),
        field(FieldType::Char, Multiplicity::Unit, "char_val"),
        field(FieldType::Int8, Multiplicity::Unit, "int8_val"),
        field(FieldType::UInt8, Multiplicity::Unit, "uint8_val"),
        field(FieldType::Int16, Multiplicity::Unit, "int16_val"),
        field(FieldType::UInt16, Multiplicity::Unit, "uint16_val"),
        field(FieldType::Int32, Multiplicity::Unit, "int32_val"),
        field(FieldType::UInt32, Multiplicity::Unit, "uint32_val"),
        field(FieldType::Int64, Multiplicity::Unit, "int64_val"),
        field(FieldType::UInt64, Multiplicity::Unit, "uint64_val"),
        field(FieldType::Float32, Multiplicity::Unit, "float32_val"),
        field(FieldType::Float64, Multiplicity::Unit, "float64_val"),
        field(FieldType::Text, Multiplicity::Unit, "string_val"),
        field(FieldType::Time, Multiplicity::Unit, "time_val"),
        field(FieldType::Duration, Multiplicity::Unit, "duration_val"),
    ];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit(&msg_spec, "all123");

    assert!(result.contains("bool bool_val{};"));
    // ROS 1: `byte` is int8 and `char` is uint8.
    assert!(result.contains("int8_t byte_val{};"));
    assert!(result.contains("uint8_t char_val{};"));
    assert!(result.contains("int8_t int8_val{};"));
    assert!(result.contains("uint8_t uint8_val{};"));
    assert!(result.contains("int16_t int16_val{};"));
    assert!(result.contains("uint16_t uint16_val{};"));
    assert!(result.contains("int32_t int32_val{};"));
    assert!(result.contains("uint32_t uint32_val{};"));
    assert!(result.contains("int64_t int64_val{};"));
    assert!(result.contains("uint64_t uint64_val{};"));
    assert!(result.contains("float float32_val{};"));
    assert!(result.contains("double float64_val{};"));
    assert!(result.contains("std::string string_val{};"));
    assert!(result.contains("hypha_msgs::RosTime time_val{};"));
    assert!(result.contains("hypha_msgs::RosDuration duration_val{};"));
}

#[test]
fn keyword_fields_all_get_underscore() {
    let type_name = TypeName::new("my_msgs", "Keywords");
    let keywords = vec![
        "class",
        "struct",
        "template",
        "typename",
        "namespace",
        "public",
        "private",
        "protected",
        "virtual",
        "operator",
        "new",
        "delete",
        "this",
        "int",
        "float",
        "double",
        "char",
        "bool",
        "void",
        "const",
        "static",
        "return",
        "if",
        "else",
        "for",
        "while",
        "switch",
        "case",
        "default",
        "break",
        "continue",
        "try",
        "catch",
        "throw",
        "using",
        "typedef",
        "union",
        "enum",
        "inline",
        "explicit",
        "friend",
        "mutable",
        "register",
        "signed",
        "unsigned",
        "short",
        "long",
        "auto",
    ];

    let fields: Vec<Field> =
        keywords.iter().map(|kw| field(FieldType::Int32, Multiplicity::Unit, kw)).collect();

    let msg_spec = spec(type_name, Vec::new(), fields);
    let result = emit(&msg_spec, "kw_all");

    for kw in keywords {
        let expected = format!("int32_t {kw}_{{}};");
        assert!(result.contains(&expected));
    }
}

#[test]
fn non_keyword_fields_unchanged() {
    let type_name = TypeName::new("my_msgs", "NonKeywords");
    let fields = vec![
        field(FieldType::Int32, Multiplicity::Unit, "my_field"),
        field(FieldType::Int32, Multiplicity::Unit, "value"),
        field(FieldType::Int32, Multiplicity::Unit, "data"),
    ];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit(&msg_spec, "nkw123");

    assert!(result.contains("int32_t my_field{};"));
    assert!(result.contains("int32_t value{};"));
    assert!(result.contains("int32_t data{};"));
}

#[test]
// The generated include order is asserted by searching for each required line.
#[allow(clippy::expect_used)]
fn nested_includes_sorted() {
    let type_name = TypeName::new("test_msgs", "MultiNested");
    let fields = vec![
        field(FieldType::Message(TypeName::new("zebra_msgs", "Zebra")), Multiplicity::Unit, "z"),
        field(FieldType::Message(TypeName::new("alpha_msgs", "Alpha")), Multiplicity::Unit, "a"),
        field(FieldType::Message(TypeName::new("bravo_msgs", "Bravo")), Multiplicity::Unit, "b"),
    ];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit_with(
        &msg_spec,
        vec![stub("zebra_msgs", "Zebra"), stub("alpha_msgs", "Alpha"), stub("bravo_msgs", "Bravo")],
        "sorted123",
    );

    let alpha_pos = result.find("alpha_msgs/Alpha.h").expect("alpha include missing");
    let bravo_pos = result.find("bravo_msgs/Bravo.h").expect("bravo include missing");
    let zebra_pos = result.find("zebra_msgs/Zebra.h").expect("zebra include missing");

    assert!(alpha_pos < bravo_pos, "alpha should come before bravo");
    assert!(bravo_pos < zebra_pos, "bravo should come before zebra");
}

#[test]
// The generated declaration order is asserted by searching for each required line.
#[allow(clippy::expect_used)]
fn constants_before_fields() {
    let type_name = TypeName::new("test_msgs", "Message");
    let constants = vec![constant(FieldType::Int32, "CONST1", "10")];
    let fields = vec![field(FieldType::Int32, Multiplicity::Unit, "field1")];
    let msg_spec = spec(type_name, constants, fields);

    let result = emit(&msg_spec, "order123");

    let const_pos = result.find("CONST1").expect("constant not found");
    let field_pos = result.find("field1").expect("field not found");

    assert!(const_pos < field_pos, "constant should come before field");
}

#[test]
fn brace_initialization_on_fields() {
    let type_name = TypeName::new("test_msgs", "Init");
    let fields = vec![
        field(FieldType::Int32, Multiplicity::Unit, "val"),
        field(FieldType::Float64, Multiplicity::Variable, "arr"),
    ];
    let msg_spec = spec(type_name, Vec::new(), fields);

    let result = emit(&msg_spec, "init123");

    assert!(result.contains("int32_t val{};"));
    assert!(result.contains("std::vector<double> arr{};"));
}

#[test]
fn integer_and_bool_constants_are_emitted_from_their_checked_value() {
    let message = spec(
        TypeName::new("my_pkg", "Limits"),
        vec![
            constant(FieldType::Int64, "LOWEST", "-9223372036854775808"),
            constant(FieldType::UInt64, "HIGHEST", "18446744073709551615"),
            constant(FieldType::UInt32, "MASK", "0x01000000"),
            constant(FieldType::Bool, "ON", "True"),
            constant(FieldType::Float64, "HALF", ".5"),
        ],
        Vec::new(),
    );
    let header = emit(&message, "00000000000000000000000000000000");
    for line in [
        "  static constexpr int64_t LOWEST = (-9223372036854775807LL - 1);\n",
        "  static constexpr uint64_t HIGHEST = 18446744073709551615ULL;\n",
        "  static constexpr uint32_t MASK = 16777216;\n",
        "  static constexpr bool ON = true;\n",
        "  static constexpr double HALF = .5;\n",
    ] {
        assert!(header.contains(line), "missing {line:?} in\n{header}");
    }
}

#[test]
fn a_hand_built_constant_out_of_range_does_not_compile() {
    let message = spec(
        TypeName::new("my_pkg", "Bad"),
        vec![constant(FieldType::Byte, "X", "200")],
        Vec::new(),
    );
    let header = emit(&message, "00000000000000000000000000000000");
    assert!(header.contains("  static_assert(false, \"constant X has no valid byte value\");\n"));
}
