//! Message definitions in, source code out.
//!
//! The emitters read a `hypha_msgdef::spec::MessageSpec` and write source; they never
//! see definition text. A build script can use the library directly:
//!
//! ```
//! use hypha_msgdef::parse::parse_message;
//! use hypha_msgdef::resolve::{message_identity, SpecSet};
//! use hypha_msgdef::spec::TypeName;
//!
//! let name = TypeName::new("demo_msgs", "Count");
//! let spec = parse_message(name.clone(), "uint32 value\n").expect("parse");
//! let mut specs = SpecSet::new();
//! specs.insert(spec);
//! let md5 = message_identity(&specs, &name).expect("identity");
//! assert_eq!(md5.len(), 32);
//! ```

pub mod emit_cpp;
pub mod emit_rust;
pub mod naming;

/// Appends formatted text. Writing to a `String` cannot fail.
fn put(output: &mut String, text: std::fmt::Arguments<'_>) {
    use std::fmt::Write as _;
    let _infallible = output.write_fmt(text);
}
