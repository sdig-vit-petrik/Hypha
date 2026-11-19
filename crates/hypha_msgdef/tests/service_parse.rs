//! ROS 1 service parsing tests.

use hypha_msgdef::parse::{ServiceParseError, parse_service};
use hypha_msgdef::spec::{FieldType, TypeName};

#[test]
#[allow(clippy::unwrap_used)]
fn service_splits_request_and_response() {
    let service = parse_service(
        TypeName::new("demo_msgs", "Add"),
        "int64 left\nint64 right\n---\nint64 sum\n",
    )
    .unwrap();
    assert_eq!(service.request.type_name.name, "AddRequest");
    assert_eq!(service.response.type_name.name, "AddResponse");
    assert_eq!(
        service.request.fields.first().map(|field| &field.field_type),
        Some(&FieldType::Int64)
    );
    assert_eq!(service.response.fields.first().map(|field| field.name.as_str()), Some("sum"));
}

#[test]
fn service_requires_exactly_one_separator() {
    let name = TypeName::new("demo_msgs", "Bad");
    assert_eq!(
        parse_service(name.clone(), "int64 value\n"),
        Err(ServiceParseError::InvalidSeparatorCount { count: 0 })
    );
    assert_eq!(
        parse_service(name, "---\n---\n"),
        Err(ServiceParseError::InvalidSeparatorCount { count: 2 })
    );
}
