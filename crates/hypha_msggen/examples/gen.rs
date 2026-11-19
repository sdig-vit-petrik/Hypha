//! Emits three messages that between them use every shape the Rust emitter
//! supports: a scalar, a time, a string, a constant, a fixed array, a nested
//! message, a variable array of a nested message, and a duration.
//!
//! Pipe the output into a crate that depends on `hypha_wire` and build it. That
//! is the check that the emitted text is a program, not only the right shape.

use hypha_msgdef::spec::{Constant, Field, FieldType, MessageSpec, Multiplicity, TypeName};
use hypha_msggen::emit_rust::emit_message;

fn field(t: FieldType, m: Multiplicity, n: &str) -> Field {
    Field { field_type: t, multiplicity: m, name: n.to_string() }
}

fn main() {
    let header = MessageSpec {
        type_name: TypeName::new("std_msgs", "Header"),
        constants: vec![],
        fields: vec![
            field(FieldType::UInt32, Multiplicity::Unit, "seq"),
            field(FieldType::Time, Multiplicity::Unit, "stamp"),
            field(FieldType::Text, Multiplicity::Unit, "frame_id"),
        ],
    };
    let point = MessageSpec {
        type_name: TypeName::new("geometry_msgs", "Point"),
        constants: vec![Constant {
            constant_type: FieldType::Int32,
            name: "ORIGIN".to_string(),
            value: "0".to_string(),
        }],
        fields: vec![
            field(FieldType::Float64, Multiplicity::Unit, "x"),
            field(FieldType::Float64, Multiplicity::Unit, "y"),
            field(FieldType::Float64, Multiplicity::Fixed(3), "extra"),
        ],
    };
    let path = MessageSpec {
        type_name: TypeName::new("nav_msgs", "Path"),
        constants: vec![],
        fields: vec![
            field(
                FieldType::Message(TypeName::new("std_msgs", "Header")),
                Multiplicity::Unit,
                "header",
            ),
            field(
                FieldType::Message(TypeName::new("geometry_msgs", "Point")),
                Multiplicity::Variable,
                "points",
            ),
            field(FieldType::Duration, Multiplicity::Unit, "timeout"),
        ],
    };
    print!("{}", emit_message(&header, "2176decaecbce78abc3b96ef049fabed"));
    print!("{}", emit_message(&point, "4a842b65f413084dc2b10fb484ea7f17"));
    print!("{}", emit_message(&path, "deadbeefdeadbeefdeadbeefdeadbeef"));
}
