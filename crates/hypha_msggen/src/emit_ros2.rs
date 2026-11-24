//! ROS 2 alias headers over the generated ROS 1 headers.
//!
//! `<pkg/msg/name.hpp>` includes `<pkg/Name.h>` and declares `pkg::msg` as a name of
//! `pkg`, for its own package and every package its fields reach, as a ROS 2 header
//! makes every type it reaches visible. A type that reaches a `time` or `duration`
//! field also gets `builtin_interfaces::msg::Time` and `Duration`, the ROS 2 names of
//! those field types. The type is the ROS 1 one: its fields, its `time` stamps
//! (`secs`, `nsecs`) and its `Ptr`, `ConstPtr`, `SharedPtr` and `UniquePtr` typedefs stay.

use std::collections::BTreeSet;

use hypha_msgdef::resolve::{ResolveError, SpecSet};
use hypha_msgdef::spec::{FieldType, MessageSpec, ServiceSpec, TypeName};

use crate::put;

/// The ROS 2 alias header of one message.
///
/// # Errors
/// Returns [`ResolveError::MissingType`] when `spec`, or a type it reaches, names a
/// type that has no definition in `specs`.
pub fn emit_message_alias(specs: &SpecSet, spec: &MessageSpec) -> Result<String, ResolveError> {
    let type_name = &spec.type_name;
    let package = &type_name.package;
    let mut output = preamble(type_name, "msg");
    put(
        &mut output,
        format_args!(
            "namespace {package} {{\nnamespace msg = ::{package};\n}}  // namespace {package}\n"
        ),
    );
    let reached = reach(specs, &[spec])?;
    emit_reached(&mut output, package, &reached);
    Ok(output)
}

/// The ROS 2 alias header of one service: `pkg::srv::Name`, with
/// `Name_Request` and `Name_Response`.
///
/// # Errors
/// Returns [`ResolveError::MissingType`] when the request or the response reaches a
/// type that has no definition in `specs`.
pub fn emit_service_alias(specs: &SpecSet, spec: &ServiceSpec) -> Result<String, ResolveError> {
    let type_name = &spec.type_name;
    let package = &type_name.package;
    let name = &type_name.name;
    let mut output = preamble(type_name, "srv");
    put(
        &mut output,
        format_args!(
            "namespace {package} {{\nnamespace srv = ::{package};\nnamespace msg = ::{package};\n\
             using {name}_Request = {name}Request;\nusing {name}_Response = {name}Response;\n\
             }}  // namespace {package}\n"
        ),
    );
    let reached = reach(specs, &[&spec.request, &spec.response])?;
    emit_reached(&mut output, package, &reached);
    Ok(output)
}

fn preamble(type_name: &TypeName, kind: &str) -> String {
    let package = &type_name.package;
    let name = &type_name.name;
    format!(
        "// Generated from the {package}/{name} definition. Do not edit.\n\
         // The ROS 2 name of <{package}/{name}.h>: {package}::{kind}::{name}.\n\n\
         #pragma once\n\n#include <{package}/{name}.h>\n\n"
    )
}

/// The `msg` names of the packages other than `own`, and the time types when reached.
fn emit_reached(output: &mut String, own: &str, reached: &Reach) {
    for package in reached.packages.iter().filter(|package| *package != own) {
        put(
            output,
            format_args!(
                "\nnamespace {package} {{\nnamespace msg = ::{package};\n}}  // namespace {package}\n"
            ),
        );
    }
    if reached.time {
        output.push_str(
            "\nnamespace builtin_interfaces::msg {\nusing Time = ::hypha_msgs::RosTime;\n\
             using Duration = ::hypha_msgs::RosDuration;\n}  // namespace builtin_interfaces::msg\n",
        );
    }
}

/// What the roots of a header reach.
struct Reach {
    /// The packages of the roots and of every type they reach, in name order.
    packages: BTreeSet<String>,
    /// Whether a `time` or `duration` field is among them.
    time: bool,
}

fn has_time_field(spec: &MessageSpec) -> bool {
    spec.fields
        .iter()
        .any(|field| matches!(field.field_type, FieldType::Time | FieldType::Duration))
}

fn reach(specs: &SpecSet, roots: &[&MessageSpec]) -> Result<Reach, ResolveError> {
    let mut packages: BTreeSet<String> =
        roots.iter().map(|root| root.type_name.package.clone()).collect();
    let mut time = roots.iter().any(|root| has_time_field(root));
    let mut seen: BTreeSet<TypeName> = BTreeSet::new();
    let mut pending: Vec<(TypeName, TypeName)> = roots
        .iter()
        .flat_map(|root| {
            root.dependencies().into_iter().map(|dependency| (dependency, root.type_name.clone()))
        })
        .collect();
    while let Some((type_name, referenced_by)) = pending.pop() {
        if !seen.insert(type_name.clone()) {
            continue;
        }
        let spec = specs.get(&type_name).ok_or_else(|| ResolveError::MissingType {
            missing: type_name.clone(),
            referenced_by,
        })?;
        packages.insert(type_name.package.clone());
        time |= has_time_field(spec);
        pending.extend(
            spec.dependencies().into_iter().map(|dependency| (dependency, type_name.clone())),
        );
    }
    Ok(Reach { packages, time })
}
