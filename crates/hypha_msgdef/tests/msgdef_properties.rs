//! Property tests of the definition parser and the walks over a parsed definition,
//! against a model built by a small grammar.

use std::collections::BTreeSet;

use hypha_msgdef::constant::{ConstantValue, constant_value};
use hypha_msgdef::definition::parse_definition;
use hypha_msgdef::md5::md5_from_definition;
use hypha_msgdef::parse::{ParseError, ServiceParseError, parse_message, parse_service};
use hypha_msgdef::resolve::{ResolveError, check_expansion, definition_text, message_identity};
use hypha_msgdef::size::{EncodedSize, SizeError, verified_encoded_size};
use hypha_msgdef::spec::{
    Constant, FieldType, MAX_EXPANDED_MESSAGES, MAX_NESTING_DEPTH, Multiplicity, TypeName,
};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Shrunk failures are kept here, relative to the crate directory, and replayed first.
const REGRESSIONS: &str = "proptest-regressions/msgdef_properties.txt";

const PACKAGE: &str = "p";

/// Miri interprets every step: a handful of cases still walks each code path.
const MIRI_CASES: u32 = 4;

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases: if cfg!(miri) { MIRI_CASES } else { cases },
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(REGRESSIONS))),
        ..ProptestConfig::default()
    }
}

/// A built-in type: its spelling, the parsed type and its fixed wire width
/// (`None` for `string`).
const BUILTINS: [(&str, FieldType, Option<usize>); 16] = [
    ("bool", FieldType::Bool, Some(1)),
    ("byte", FieldType::Byte, Some(1)),
    ("char", FieldType::Char, Some(1)),
    ("int8", FieldType::Int8, Some(1)),
    ("uint8", FieldType::UInt8, Some(1)),
    ("int16", FieldType::Int16, Some(2)),
    ("uint16", FieldType::UInt16, Some(2)),
    ("int32", FieldType::Int32, Some(4)),
    ("uint32", FieldType::UInt32, Some(4)),
    ("int64", FieldType::Int64, Some(8)),
    ("uint64", FieldType::UInt64, Some(8)),
    ("float32", FieldType::Float32, Some(4)),
    ("float64", FieldType::Float64, Some(8)),
    ("string", FieldType::Text, None),
    ("time", FieldType::Time, Some(8)),
    ("duration", FieldType::Duration, Some(8)),
];

/// An integer constant type: its spelling, the parsed type and its range.
const INTEGERS: [(&str, FieldType, i128, i128); 10] = [
    ("byte", FieldType::Byte, -128, 127),
    ("char", FieldType::Char, 0, 255),
    ("int8", FieldType::Int8, -128, 127),
    ("uint8", FieldType::UInt8, 0, 255),
    ("int16", FieldType::Int16, -32_768, 32_767),
    ("uint16", FieldType::UInt16, 0, 65_535),
    ("int32", FieldType::Int32, -2_147_483_648, 2_147_483_647),
    ("uint32", FieldType::UInt32, 0, 4_294_967_295),
    ("int64", FieldType::Int64, -9_223_372_036_854_775_808, 9_223_372_036_854_775_807),
    ("uint64", FieldType::UInt64, 0, 18_446_744_073_709_551_615),
];

/// The type of a generated field.
#[derive(Debug, Clone)]
enum FieldKind {
    /// An index into [`BUILTINS`].
    Builtin(usize),
    /// A later type of the model; `qualified` writes it as `p/Tj` rather than `Tj`.
    Nested { target: usize, qualified: bool },
}

/// One generated line of a message body.
#[derive(Debug, Clone)]
enum Line {
    Field {
        kind: FieldKind,
        multiplicity: Multiplicity,
        comment: Option<String>,
    },
    /// A constant: its spelling, its parsed type, its value as written and a comment.
    Constant {
        type_text: &'static str,
        constant_type: FieldType,
        value: String,
        comment: Option<String>,
    },
    Comment(String),
    Blank(String),
}

/// One generated definition: its types `T0` (the root) to `Tn`, and the text layout.
#[derive(Debug, Clone)]
struct Model {
    types: Vec<Vec<Line>>,
    separator_width: usize,
    crlf: bool,
    reverse_sections: bool,
    stray_line: bool,
    gap: &'static str,
}

fn comment() -> impl Strategy<Value = String> {
    "[ -~]{0,16}".prop_map(|text| format!("#{text}"))
}

fn multiplicity() -> impl Strategy<Value = Multiplicity> {
    prop_oneof![
        3 => Just(Multiplicity::Unit),
        1 => Just(Multiplicity::Variable),
        1 => (0..4_usize).prop_map(Multiplicity::Fixed),
    ]
}

fn field_kind() -> impl Strategy<Value = FieldKind> {
    prop_oneof![
        (0..BUILTINS.len()).prop_map(FieldKind::Builtin),
        (any::<usize>(), any::<bool>())
            .prop_map(|(target, qualified)| FieldKind::Nested { target, qualified }),
    ]
}

/// An integer literal in range for one of [`INTEGERS`].
fn integer_constant() -> impl Strategy<Value = (&'static str, FieldType, String)> {
    (0..INTEGERS.len(), 0..4_u8, any::<u64>(), any::<bool>(), any::<bool>()).prop_map(
        |(index, pick, random, hex, plus)| {
            let (text, constant_type, min, max) =
                INTEGERS.get(index).cloned().unwrap_or(INTEGERS[0].clone());
            let span = max.saturating_sub(min).saturating_add(1);
            let value = match pick {
                0 => min,
                1 => max,
                2 => 0,
                _ => min.saturating_add(i128::from(random).rem_euclid(span)),
            };
            (text, constant_type, integer_text(value, hex, plus))
        },
    )
}

fn integer_text(value: i128, hex: bool, plus: bool) -> String {
    let sign = if value < 0 {
        "-"
    } else if plus {
        "+"
    } else {
        ""
    };
    let magnitude = value.unsigned_abs();
    if hex { format!("{sign}0x{magnitude:X}") } else { format!("{sign}{magnitude}") }
}

fn constant_line() -> impl Strategy<Value = Line> {
    let integer = integer_constant();
    let boolean =
        prop_oneof![Just("true"), Just("false"), Just("True"), Just("False"), Just("1"), Just("0")]
            .prop_map(|text| ("bool", FieldType::Bool, String::from(text)));
    let float64 = any::<f64>()
        .prop_filter("finite", |value| value.is_finite())
        .prop_map(|value| ("float64", FieldType::Float64, format!("{value:?}")));
    let float32 = any::<f32>()
        .prop_filter("finite", |value| value.is_finite())
        .prop_map(|value| ("float32", FieldType::Float32, format!("{value}")));
    // A string constant is the rest of the line: `#` and `=` are part of it.
    let text = "[ -~]{0,24}".prop_map(|value| ("string", FieldType::Text, value));
    (
        prop_oneof![4 => integer, 1 => boolean, 1 => float64, 1 => float32, 1 => text],
        proptest::option::of(comment()),
    )
        .prop_map(|((type_text, constant_type, value), comment)| {
            let comment = if constant_type == FieldType::Text { None } else { comment };
            Line::Constant { type_text, constant_type, value, comment }
        })
}

fn line() -> impl Strategy<Value = Line> {
    prop_oneof![
        6 => (field_kind(), multiplicity(), proptest::option::of(comment()))
            .prop_map(|(kind, multiplicity, comment)| Line::Field { kind, multiplicity, comment }),
        3 => constant_line(),
        1 => comment().prop_map(Line::Comment),
        1 => "[ \t]{0,3}".prop_map(Line::Blank),
    ]
}

fn model() -> impl Strategy<Value = Model> {
    (
        vec(vec(line(), 0..8), 1..7),
        1..100_usize,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prop_oneof![Just(" "), Just("\t"), Just("   ")],
    )
        .prop_map(|(types, separator_width, crlf, reverse_sections, stray_line, gap)| Model {
            types,
            separator_width,
            crlf,
            reverse_sections,
            stray_line,
            gap,
        })
}

fn type_name(index: usize) -> TypeName {
    TypeName::new(PACKAGE, &format!("T{index}"))
}

/// The type a nested field of type `owner` points at, if a later type exists.
fn nested_target(model: &Model, owner: usize, target: usize) -> Option<usize> {
    let first = owner.checked_add(1)?;
    let later = model.types.len().checked_sub(first).filter(|&count| count > 0)?;
    first.checked_add(target.checked_rem(later)?)
}

/// What one field parses to: its type, and its fixed width when it has one.
fn field_type(model: &Model, owner: usize, kind: &FieldKind) -> (String, FieldType) {
    match kind {
        FieldKind::Builtin(index) => {
            let (text, field_type, _) =
                BUILTINS.get(*index).cloned().unwrap_or(BUILTINS[0].clone());
            (String::from(text), field_type)
        }
        FieldKind::Nested { target, qualified } => match nested_target(model, owner, *target) {
            Some(nested) => {
                let spelled =
                    if *qualified { format!("{PACKAGE}/T{nested}") } else { format!("T{nested}") };
                (spelled, FieldType::Message(type_name(nested)))
            }
            None => (String::from("int32"), FieldType::Int32),
        },
    }
}

/// The body of type `owner` as definition text lines.
fn render_type(model: &Model, owner: usize) -> Vec<String> {
    let lines = model.types.get(owner).map_or(&[][..], Vec::as_slice);
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| match line {
            Line::Field { kind, multiplicity, comment } => {
                let (spelled, _) = field_type(model, owner, kind);
                let trailing = comment.as_ref().map_or(String::new(), |text| format!(" {text}"));
                format!("{spelled}{}{}f{index}{trailing}", multiplicity.render(), model.gap)
            }
            Line::Constant { type_text, value, comment, .. } => {
                let trailing = comment.as_ref().map_or(String::new(), |text| format!(" {text}"));
                format!("{type_text}{}C{index}={value}{trailing}", model.gap)
            }
            Line::Comment(text) => format!("  {text}"),
            Line::Blank(text) => text.clone(),
        })
        .collect()
}

/// The whole definition text: the root body, then one `MSG:` section per later type.
fn render(model: &Model) -> String {
    let mut lines = render_type(model, 0);
    let mut later: Vec<usize> = (1..model.types.len()).collect();
    if model.reverse_sections {
        later.reverse();
    }
    for index in later {
        lines.push("=".repeat(model.separator_width));
        if model.stray_line {
            lines.push(String::from("# stray"));
        }
        lines.push(format!("MSG: {PACKAGE}/T{index}"));
        lines.extend(render_type(model, index));
    }
    let newline = if model.crlf { "\r\n" } else { "\n" };
    lines.join(newline)
}

/// Every type the root reaches, the root included.
fn reachable(model: &Model) -> BTreeSet<usize> {
    let mut seen = BTreeSet::from([0]);
    let mut pending = vec![0];
    while let Some(owner) = pending.pop() {
        for line in model.types.get(owner).map_or(&[][..], Vec::as_slice) {
            if let Line::Field { kind: FieldKind::Nested { target, .. }, .. } = line
                && let Some(nested) = nested_target(model, owner, *target)
                && seen.insert(nested)
            {
                pending.push(nested);
            }
        }
    }
    seen
}

/// The fixed encoded size of type `owner`, or `None` when it varies.
fn model_size(model: &Model, owner: usize) -> Option<usize> {
    let mut total = 0_usize;
    for line in model.types.get(owner).map_or(&[][..], Vec::as_slice) {
        let Line::Field { kind, multiplicity, .. } = line else {
            continue;
        };
        let count = match multiplicity {
            Multiplicity::Unit => 1,
            Multiplicity::Fixed(count) => *count,
            Multiplicity::Variable => return None,
        };
        let width = match kind {
            FieldKind::Builtin(index) => BUILTINS.get(*index).and_then(|builtin| builtin.2)?,
            FieldKind::Nested { target, .. } => match nested_target(model, owner, *target) {
                Some(nested) => model_size(model, nested)?,
                None => 4,
            },
        };
        total = total.checked_add(width.checked_mul(count)?)?;
    }
    Some(total)
}

/// The value text a constant keeps: a `string` keeps the rest of the line, others
/// stop at a `#`; both are trimmed.
fn kept_value(line: &Line) -> Option<(FieldType, String)> {
    let Line::Constant { constant_type, value, .. } = line else {
        return None;
    };
    Some((constant_type.clone(), value.trim().to_string()))
}

/// A chain of `depth` types where each holds `fan` fields of the next, as a unit or
/// a fixed array of `length`; the last holds one `int32`.
fn ladder(depth: usize, fan: usize, length: usize) -> String {
    let suffix = if length == 1 { String::new() } else { format!("[{length}]") };
    let bodies: Vec<String> = (0..depth)
        .map(|level| {
            let next = level.saturating_add(1);
            let body = if next == depth {
                String::from("int32 leaf")
            } else {
                let fields: Vec<String> =
                    (0..fan).map(|field| format!("L{next}{suffix} f{field}")).collect();
                fields.join("\n")
            };
            if level == 0 {
                body
            } else {
                format!("{}\nMSG: {PACKAGE}/L{level}\n{body}", "=".repeat(80))
            }
        })
        .collect();
    bodies.join("\n")
}

/// The message instances of the ladder's root: `1 + fan*length*(1 + ...)`, saturating.
fn ladder_instances(depth: usize, fan: usize, length: usize) -> u64 {
    let per_level = u64::try_from(fan.saturating_mul(length)).unwrap_or(u64::MAX);
    (1..depth).fold(1_u64, |below, _| per_level.saturating_mul(below).saturating_add(1))
}

/// Text built from the pieces the grammar cares about.
fn token_soup() -> impl Strategy<Value = String> {
    let token = prop_oneof![
        Just("int32"),
        Just("string"),
        Just("Header"),
        Just("p/T"),
        Just("/"),
        Just("T"),
        Just("["),
        Just("]"),
        Just("[3]"),
        Just("[]"),
        Just("="),
        Just("#"),
        Just(" "),
        Just("\t"),
        Just("\n"),
        Just("\r\n"),
        Just("---"),
        Just("MSG: p/T"),
        Just("====="),
        Just("0x"),
        Just("-"),
        Just("+"),
        Just("1e5"),
        Just("."),
        Just("99999999999999999999999"),
        Just("time"),
        Just("é"),
        Just("\u{85}"),
        Just("\u{a0}"),
    ];
    vec(token, 0..48).prop_map(|tokens| tokens.concat())
}

proptest! {
    #![proptest_config(config(256))]

    #[test]
    fn a_generated_definition_parses_to_its_model(model in model()) {
        let text = render(&model);
        let specs = parse_definition(&type_name(0), &text)
            .map_err(|error| TestCaseError::fail(format!("{error}\n{text}")))?;
        for (owner, lines) in model.types.iter().enumerate() {
            let spec = specs.get(&type_name(owner)).ok_or_else(|| TestCaseError::fail(format!("T{owner} missing")))?;
            let mut fields = spec.fields.iter();
            let mut constants = spec.constants.iter();
            for (index, line) in lines.iter().enumerate() {
                match line {
                    Line::Field { kind, multiplicity, .. } => {
                        let field = fields.next().ok_or_else(|| TestCaseError::fail("a field is missing"))?;
                        prop_assert_eq!(&field.field_type, &field_type(&model, owner, kind).1);
                        prop_assert_eq!(field.multiplicity, *multiplicity);
                        prop_assert_eq!(&field.name, &format!("f{index}"));
                    }
                    Line::Constant { .. } => {
                        let constant = constants.next().ok_or_else(|| TestCaseError::fail("a constant is missing"))?;
                        let (constant_type, value) = kept_value(line).unwrap_or((FieldType::Bool, String::new()));
                        prop_assert_eq!(constant, &Constant { constant_type, name: format!("C{index}"), value });
                    }
                    Line::Comment(_) | Line::Blank(_) => {}
                }
            }
            prop_assert!(fields.next().is_none(), "T{} has an extra field", owner);
            prop_assert!(constants.next().is_none(), "T{} has an extra constant", owner);
        }
        prop_assert_eq!(specs.values().count(), model.types.len());
    }

    #[test]
    fn definition_text_is_a_fixed_point_and_keeps_the_identity(model in model()) {
        let root = type_name(0);
        let text = render(&model);
        let specs = parse_definition(&root, &text).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let canonical = definition_text(&specs, &root).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let reparsed = parse_definition(&root, &canonical).map_err(|error| TestCaseError::fail(format!("{error}\n{canonical}")))?;
        let rendered_again = definition_text(&reparsed, &root).ok();
        prop_assert_eq!(rendered_again.as_deref(), Some(canonical.as_str()));

        let identity = message_identity(&specs, &root).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let lower_hex = identity.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        prop_assert!(identity.len() == 32 && lower_hex, "identity {}", identity);
        prop_assert_eq!(message_identity(&reparsed, &root).ok(), Some(identity.clone()));
        prop_assert_eq!(md5_from_definition("p/T0", &text), Some(identity.clone()));
        prop_assert_eq!(md5_from_definition("p/T0", &canonical), Some(identity));

        // Only the types the root reaches are in the text, each once.
        let sections = canonical.matches("\nMSG: ").count();
        prop_assert_eq!(sections, reachable(&model).len().saturating_sub(1));
    }

    #[test]
    fn the_fixed_size_is_the_sum_of_the_field_widths(model in model()) {
        let text = render(&model);
        let md5 = md5_from_definition("p/T0", &text).ok_or_else(|| TestCaseError::fail("no identity"))?;
        let expected = match model_size(&model, 0) {
            Some(size) => EncodedSize::Fixed(size),
            None => EncodedSize::Variable,
        };
        prop_assert_eq!(verified_encoded_size("p/T0", &md5, &text), Ok(expected));
        let mut wrong = md5.clone();
        wrong.replace_range(..1, if md5.starts_with('0') { "1" } else { "0" });
        let refused = matches!(verified_encoded_size("p/T0", &wrong, &text), Err(SizeError::Md5Mismatch { .. }));
        prop_assert!(refused);
    }

    #[test]
    fn a_service_is_its_two_halves(request in model(), response in model()) {
        let request_text = render_type(&request, 0).join("\n");
        let response_text = render_type(&response, 0).join("\n");
        let text = format!("{request_text}\n---\n{response_text}");
        let service = parse_service(TypeName::new(PACKAGE, "S"), &text)
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        prop_assert_eq!(Ok(service.request), parse_message(TypeName::new(PACKAGE, "SRequest"), &request_text));
        prop_assert_eq!(Ok(service.response), parse_message(TypeName::new(PACKAGE, "SResponse"), &response_text));
        let twice = format!("{text}\n---\n");
        let refused = matches!(parse_service(TypeName::new(PACKAGE, "S"), &twice), Err(ServiceParseError::InvalidSeparatorCount { count: 2 }));
        prop_assert!(refused);
    }

    #[test]
    fn an_integer_constant_is_kept_exactly_in_range_and_refused_outside(
        index in 0..INTEGERS.len(),
        offset in 0..1000_i128,
        hex in any::<bool>(),
    ) {
        let (text, constant_type, min, max) = INTEGERS.get(index).cloned().unwrap_or(INTEGERS[0].clone());
        for value in [min, max, min.saturating_add(offset).min(max), max.saturating_sub(offset).max(min)] {
            let constant = Constant { constant_type: constant_type.clone(), name: String::from("C"), value: integer_text(value, hex, false) };
            let expected = if min < 0 {
                i64::try_from(value).ok().map(ConstantValue::Signed)
            } else {
                u64::try_from(value).ok().map(ConstantValue::Unsigned)
            };
            prop_assert_eq!(constant_value(&constant).ok(), expected);
        }
        for value in [min.saturating_sub(1).saturating_sub(offset), max.saturating_add(1).saturating_add(offset)] {
            let line = format!("{text} C={}", integer_text(value, hex, false));
            let refused = matches!(parse_message(type_name(0), &line), Err(ParseError::ConstantOutOfRange { .. }));
            prop_assert!(refused, "{} was accepted", line);
        }
    }

    #[test]
    fn any_text_is_parsed_or_refused_without_a_panic(
        text in prop_oneof![token_soup(), "\\PC{0,64}", any::<String>()],
    ) {
        let root = type_name(0);
        let _ = parse_message(root.clone(), &text);
        let _ = parse_service(root.clone(), &text);
        if let Ok(specs) = parse_definition(&root, &text) {
            let _ = check_expansion(&specs, &root);
            let canonical = definition_text(&specs, &root);
            let identity = message_identity(&specs, &root);
            // Whatever parses and resolves renders to text that parses to itself, with
            // the same identity.
            if let (Ok(canonical), Ok(identity)) = (canonical, identity) {
                prop_assert_eq!(md5_from_definition("p/T0", &canonical), Some(identity));
                let reparsed = parse_definition(&root, &canonical)
                    .map_err(|error| TestCaseError::fail(format!("{error}\n{canonical}")))?;
                let rendered_again = definition_text(&reparsed, &root).ok();
                prop_assert_eq!(rendered_again.as_deref(), Some(canonical.as_str()));
            }
        }
        let _ = md5_from_definition("p/T0", &text);
        let _ = verified_encoded_size("p/T0", "00000000000000000000000000000000", &text);
    }
}

proptest! {
    #![proptest_config(config(128))]

    #[test]
    fn the_depth_and_expansion_limits_refuse_exactly_what_passes_them(
        // The limits are hit at their boundaries: a depth of the limit and one past
        // it, and `1 + fan * length` instances on either side of the expansion limit.
        depth in prop_oneof![
            1..(MAX_NESTING_DEPTH + 16),
            Just(MAX_NESTING_DEPTH),
            Just(MAX_NESTING_DEPTH + 1),
            Just(2_usize),
        ],
        fan in 1..3_usize,
        length in prop_oneof![
            Just(1_usize),
            0..6_usize,
            32_766..32_770_usize,
            65_534..65_538_usize,
        ],
    ) {
        let text = ladder(depth, fan, length);
        let root = TypeName::new(PACKAGE, "L0");
        let specs = parse_definition(&root, &text).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let instances = ladder_instances(depth, fan, length);
        let result = check_expansion(&specs, &root);
        let size = verified_encoded_size("p/L0", "00000000000000000000000000000000", &text);
        if depth > MAX_NESTING_DEPTH {
            let too_deep = matches!(result, Err(ResolveError::NestingTooDeep { limit: MAX_NESTING_DEPTH, .. }));
            prop_assert!(too_deep, "{:?}", result);
            prop_assert_eq!(size, Err(SizeError::NestingTooDeep { limit: MAX_NESTING_DEPTH }));
        } else if instances > MAX_EXPANDED_MESSAGES {
            let too_large = matches!(result, Err(ResolveError::TooLarge { .. }));
            prop_assert!(too_large, "{} instances: {:?}", instances, result);
            prop_assert_eq!(size, Err(SizeError::TooLarge));
            prop_assert_eq!(md5_from_definition("p/L0", &text), None);
        } else {
            prop_assert_eq!(result, Ok(()));
            let md5 = md5_from_definition("p/L0", &text).ok_or_else(|| TestCaseError::fail("no identity"))?;
            // Each instance below the root is one level of fields; the leaf holds 4 bytes.
            let leaves = u64::try_from(fan.saturating_mul(length)).unwrap_or(u64::MAX)
                .saturating_pow(u32::try_from(depth.saturating_sub(1)).unwrap_or(u32::MAX));
            let expected = usize::try_from(leaves.saturating_mul(4)).unwrap_or(usize::MAX);
            prop_assert_eq!(verified_encoded_size("p/L0", &md5, &text), Ok(EncodedSize::Fixed(expected)));
        }
    }
}
