//! ROS 2 alias headers: `<pkg/msg/name.hpp>` and `pkg::msg::Name` over the ROS 1 header.

use hypha_msgdef::parse::{parse_message, parse_service};
use hypha_msgdef::resolve::SpecSet;
use hypha_msgdef::spec::TypeName;
use hypha_msggen::emit_ros2::{emit_message_alias, emit_service_alias};

#[allow(clippy::expect_used)]
fn specs_with(definitions: &[(&str, &str, &str)]) -> SpecSet {
    let mut specs = SpecSet::new();
    for (package, name, text) in definitions {
        specs.insert(parse_message(TypeName::new(package, name), text).expect("parses"));
    }
    specs
}

#[test]
#[allow(clippy::expect_used)]
fn message_alias_names_every_package_the_type_reaches() {
    let specs = specs_with(&[
        ("std_msgs", "Header", "uint32 seq\ntime stamp\nstring frame_id\n"),
        ("geometry_msgs", "Point", "float64 x\n"),
        ("geometry_msgs", "Pose", "geometry_msgs/Point position\n"),
        ("demo_msgs", "Target", "std_msgs/Header header\ngeometry_msgs/Pose pose\n"),
    ]);
    let target = specs.get(&TypeName::new("demo_msgs", "Target")).expect("target");
    let output = emit_message_alias(&specs, target).expect("alias emits");
    let expected = concat!(
        "// Generated from the demo_msgs/Target definition. Do not edit.\n",
        "// The ROS 2 name of <demo_msgs/Target.h>: demo_msgs::msg::Target.\n",
        "\n",
        "#pragma once\n",
        "\n",
        "#include <demo_msgs/Target.h>\n",
        "\n",
        "namespace demo_msgs {\n",
        "namespace msg = ::demo_msgs;\n",
        "}  // namespace demo_msgs\n",
        "\n",
        "namespace geometry_msgs {\n",
        "namespace msg = ::geometry_msgs;\n",
        "}  // namespace geometry_msgs\n",
        "\n",
        "namespace std_msgs {\n",
        "namespace msg = ::std_msgs;\n",
        "}  // namespace std_msgs\n",
        "\n",
        "namespace builtin_interfaces::msg {\n",
        "using Time = ::hypha_msgs::RosTime;\n",
        "using Duration = ::hypha_msgs::RosDuration;\n",
        "}  // namespace builtin_interfaces::msg\n",
    );
    assert_eq!(output, expected);
}

#[test]
#[allow(clippy::expect_used)]
fn message_alias_refuses_a_type_without_definition() {
    let specs = specs_with(&[("demo_msgs", "Target", "geometry_msgs/Pose pose\n")]);
    let target = specs.get(&TypeName::new("demo_msgs", "Target")).expect("target");
    assert!(emit_message_alias(&specs, target).is_err());
}

#[test]
#[allow(clippy::expect_used)]
fn service_alias_declares_the_srv_name_and_its_halves() {
    let service = parse_service(
        TypeName::new("demo_msgs", "Move"),
        "geometry_msgs/Point target\n---\nbool done\n",
    )
    .expect("service parses");
    let mut specs = specs_with(&[("geometry_msgs", "Point", "float64 x\n")]);
    specs.insert(service.request.clone());
    specs.insert(service.response.clone());
    let output = emit_service_alias(&specs, &service).expect("alias emits");
    let expected = concat!(
        "// Generated from the demo_msgs/Move definition. Do not edit.\n",
        "// The ROS 2 name of <demo_msgs/Move.h>: demo_msgs::srv::Move.\n",
        "\n",
        "#pragma once\n",
        "\n",
        "#include <demo_msgs/Move.h>\n",
        "\n",
        "namespace demo_msgs {\n",
        "namespace srv = ::demo_msgs;\n",
        "namespace msg = ::demo_msgs;\n",
        "using Move_Request = MoveRequest;\n",
        "using Move_Response = MoveResponse;\n",
        "}  // namespace demo_msgs\n",
        "\n",
        "namespace geometry_msgs {\n",
        "namespace msg = ::geometry_msgs;\n",
        "}  // namespace geometry_msgs\n",
    );
    assert_eq!(output, expected);
}
