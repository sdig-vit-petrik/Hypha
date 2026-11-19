//! The shape of one parsed message definition.
//!
//! `byte` and `char` are kept apart from `int8` and `uint8`: the identity hashes the
//! type names as written.

/// The deepest message nesting a walker over a definition follows, with the root
/// message counted as level 1.
///
/// A definition that nests deeper, or recurses through itself, is refused.
pub const MAX_NESTING_DEPTH: usize = 64;

/// The most message instances a definition may expand to: the root, plus for each
/// message field its count times the instances of its type (a fixed array counts
/// its length, a variable one counts 1, its wire count being bounded by the bytes).
///
/// A definition past it is refused, so every walker is bounded.
pub const MAX_EXPANDED_MESSAGES: u64 = 65_536;

/// A message type, named by its package and its own name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeName {
    /// The package the type belongs to, such as `std_msgs`.
    pub package: String,
    /// The type's own name, such as `Header`.
    pub name: String,
}

impl TypeName {
    /// A type name from its two parts.
    #[must_use]
    pub fn new(package: &str, name: &str) -> Self {
        Self { package: String::from(package), name: String::from(name) }
    }

    /// Splits `package/Name` at its first `/`, if both parts are non-empty.
    pub(crate) fn parse(full_name: &str) -> Option<Self> {
        let (package, name) =
            full_name.split_once('/').filter(|(p, n)| !p.is_empty() && !n.is_empty())?;
        Some(Self::new(package, name))
    }

    /// The full name, `package/Name`.
    #[must_use]
    pub fn full(&self) -> String {
        format!("{}/{}", self.package, self.name)
    }
}

/// What one value of a field holds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldType {
    /// One byte, `0` or `1`.
    Bool,
    /// A deprecated spelling of a signed byte, as in ROS 1 (`byte` is `int8`).
    Byte,
    /// A deprecated spelling of an unsigned byte, as in ROS 1 (`char` is `uint8`).
    Char,
    /// A signed 8-bit integer.
    Int8,
    /// An unsigned 8-bit integer.
    UInt8,
    /// A signed 16-bit integer.
    Int16,
    /// An unsigned 16-bit integer.
    UInt16,
    /// A signed 32-bit integer.
    Int32,
    /// An unsigned 32-bit integer.
    UInt32,
    /// A signed 64-bit integer.
    Int64,
    /// An unsigned 64-bit integer.
    UInt64,
    /// A 32-bit float.
    Float32,
    /// A 64-bit float.
    Float64,
    /// A length-prefixed byte string.
    Text,
    /// A point in time: two unsigned 32-bit values.
    Time,
    /// A span of time: two signed 32-bit values.
    Duration,
    /// Another message, named in full.
    Message(TypeName),
}

impl FieldType {
    /// The type name as it appears in definition text.
    #[must_use]
    pub fn render(&self) -> String {
        let builtin = match self {
            Self::Bool => "bool",
            Self::Byte => "byte",
            Self::Char => "char",
            Self::Int8 => "int8",
            Self::UInt8 => "uint8",
            Self::Int16 => "int16",
            Self::UInt16 => "uint16",
            Self::Int32 => "int32",
            Self::UInt32 => "uint32",
            Self::Int64 => "int64",
            Self::UInt64 => "uint64",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
            Self::Text => "string",
            Self::Time => "time",
            Self::Duration => "duration",
            Self::Message(name) => return name.full(),
        };
        String::from(builtin)
    }

    /// True when the type is built in, so it carries no identity of its own.
    #[must_use]
    pub const fn is_builtin(&self) -> bool {
        !matches!(self, Self::Message(_))
    }
}

/// How many values a field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Multiplicity {
    /// Exactly one value, written with no brackets.
    Unit,
    /// A fixed count, written `[n]`. The count never crosses the wire.
    Fixed(usize),
    /// Any count, written `[]`. A `u32` count precedes the values.
    Variable,
}

impl Multiplicity {
    /// The bracket suffix as it appears in definition text.
    #[must_use]
    pub fn render(self) -> String {
        match self {
            Self::Unit => String::new(),
            Self::Variable => String::from("[]"),
            Self::Fixed(count) => format!("[{count}]"),
        }
    }
}

/// One field of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The field's type.
    pub field_type: FieldType,
    /// How many values it holds.
    pub multiplicity: Multiplicity,
    /// The field's name.
    pub name: String,
}

impl Field {
    /// The type and brackets as they appear in definition text, such as
    /// `float64[3]`.
    #[must_use]
    pub fn render_type(&self) -> String {
        format!("{}{}", self.field_type.render(), self.multiplicity.render())
    }
}

/// One named constant of a message.
///
/// A constant never crosses the wire. It takes part in the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constant {
    /// The constant's type. Always built in.
    pub constant_type: FieldType,
    /// The constant's name.
    pub name: String,
    /// The value, kept as written.
    pub value: String,
}

/// One parsed message definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSpec {
    /// The type this definition describes.
    pub type_name: TypeName,
    /// The constants, in the order they were written.
    pub constants: Vec<Constant>,
    /// The fields, in the order they were written. Wire order follows this.
    pub fields: Vec<Field>,
}

impl MessageSpec {
    /// Every message type this definition names directly, in field order.
    ///
    /// A type that appears twice is returned twice.
    #[must_use]
    pub fn dependencies(&self) -> Vec<TypeName> {
        let nested = self.fields.iter().filter_map(|field| match &field.field_type {
            FieldType::Message(name) => Some(name.clone()),
            _ => None,
        });
        nested.collect()
    }
}

/// The request and response parsed from one ROS 1 service definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    /// The service type, such as `demo_msgs/Compute`.
    pub type_name: TypeName,
    /// The fields before the `---` separator.
    pub request: MessageSpec,
    /// The fields after the `---` separator.
    pub response: MessageSpec,
}
