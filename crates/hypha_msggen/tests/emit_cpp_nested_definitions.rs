//! C++ definition text for nested message types, in messages and services.

use hypha_msgdef::parse::{parse_message, parse_service};
use hypha_msgdef::resolve::{SpecSet, message_identity, service_identity};
use hypha_msgdef::spec::TypeName;
use hypha_msggen::emit_cpp::{emit_message, emit_service};

#[test]
#[allow(clippy::expect_used)]
fn service_request_definition_includes_nested_message_sections() {
    let service = parse_service(
        TypeName::new("demo_msgs", "Move"),
        "geometry_msgs/Point target\n---\nbool done\n",
    )
    .expect("service parses");
    let point =
        parse_message(TypeName::new("geometry_msgs", "Point"), "float64 x\nfloat64 y\nfloat64 z\n")
            .expect("point parses");
    let mut specs = SpecSet::new();
    specs.insert(point);
    specs.insert(service.request.clone());
    specs.insert(service.response.clone());
    let identity = service_identity(&specs, &service).expect("identity resolves");
    let output = emit_service(&specs, &service, &identity).expect("service emits");
    let expected_definition = r#"    return "geometry_msgs/Point target\n"
    "================================================================================\n"
    "MSG: geometry_msgs/Point\n"
    "float64 x\n"
    "float64 y\n"
    "float64 z";"#;
    assert!(output.request.contains(expected_definition));
    assert!(output.request.contains(&format!("MD5 = \"{}\";", identity.request)));
    assert!(output.response.contains(&format!("MD5 = \"{}\";", identity.response)));
}

#[test]
#[allow(clippy::expect_used)]
fn each_service_half_is_emitted_with_its_own_md5() {
    let service =
        parse_service(TypeName::new("demo_msgs", "Scale"), "float64 k\n---\nfloat64 out\n")
            .expect("service parses");
    let mut specs = SpecSet::new();
    specs.insert(service.request.clone());
    specs.insert(service.response.clone());
    let identity = service_identity(&specs, &service).expect("identity resolves");
    let output = emit_service(&specs, &service, &identity).expect("service emits");
    let request_md5 = message_identity(&specs, &service.request.type_name).expect("request md5");
    let response_md5 = message_identity(&specs, &service.response.type_name).expect("response md5");
    assert_eq!(identity.request, request_md5);
    assert_eq!(identity.response, response_md5);
    assert!(output.request.contains(&format!("MD5 = \"{request_md5}\";")));
    assert!(output.response.contains(&format!("MD5 = \"{response_md5}\";")));
    assert!(output.service.contains(&format!("MD5 = \"{}\";", identity.service)));
    assert_ne!(identity.service, identity.request, "the half MD5 is not the service MD5");
}

#[test]
#[allow(clippy::expect_used)]
fn message_with_nested_type_includes_nested_message_sections() {
    let point = parse_message(TypeName::new("geometry_msgs", "Point"), "float64 x\nfloat64 y\n")
        .expect("point parses");
    let outer = parse_message(TypeName::new("demo_msgs", "Outer"), "geometry_msgs/Point point\n")
        .expect("message parses");
    let mut specs = SpecSet::new();
    specs.insert(point);
    specs.insert(outer.clone());
    let output = emit_message(&specs, &outer, "md5").expect("message emits");
    assert!(output.contains("MSG: geometry_msgs/Point"));
}
