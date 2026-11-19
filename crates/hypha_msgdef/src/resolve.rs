//! Dependency order, concatenated definition text, and message identity.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use crate::md5::md5_hex;
use crate::spec::{
    Constant, FieldType, MAX_EXPANDED_MESSAGES, MAX_NESTING_DEPTH, MessageSpec, Multiplicity,
    ServiceSpec, TypeName,
};

/// The level of the root message, the top of every walk over the graph.
pub(crate) const ROOT_LEVEL: usize = 1;

/// The width, in `=` characters, of the separator before each non-root section.
const SEPARATOR_WIDTH: usize = 80;

/// A set of message specifications that can be looked up by type name.
#[derive(Debug, Clone)]
pub struct SpecSet {
    specs: HashMap<TypeName, MessageSpec>,
}

impl SpecSet {
    /// Creates an empty set.
    #[must_use]
    pub fn new() -> Self {
        Self { specs: HashMap::new() }
    }

    /// Adds `spec`, keyed by its own type name.
    /// A later insert with the same type name replaces the earlier one.
    pub fn insert(&mut self, spec: MessageSpec) {
        self.specs.insert(spec.type_name.clone(), spec);
    }

    /// Looks up the specification for `name`, if the set holds one.
    #[must_use]
    pub fn get(&self, name: &TypeName) -> Option<&MessageSpec> {
        self.specs.get(name)
    }

    /// Returns an iterator over the message specifications in the set.
    pub fn values(&self) -> impl Iterator<Item = &MessageSpec> {
        self.specs.values()
    }
}

impl Default for SpecSet {
    fn default() -> Self {
        Self::new()
    }
}

/// A problem found while resolving a set of message specifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// A type was referenced but has no definition in the [`SpecSet`].
    MissingType {
        /// The type that has no definition.
        missing: TypeName,
        /// The type whose field named the missing type.
        referenced_by: TypeName,
    },
    /// A type depends on itself, directly or through other types.
    Cycle {
        /// The type whose field points back to an ancestor on the current path.
        at: TypeName,
    },
    /// A chain of nested types is deeper than [`MAX_NESTING_DEPTH`].
    NestingTooDeep {
        /// The deepest nesting allowed.
        limit: usize,
        /// The first type that would sit below the limit.
        at: TypeName,
    },
    /// The definition expands to more message instances than the shared limit.
    TooLarge {
        /// The most instances allowed, [`MAX_EXPANDED_MESSAGES`](crate::spec::MAX_EXPANDED_MESSAGES).
        limit: u64,
        /// The type whose expansion passes the limit.
        at: TypeName,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingType { missing, referenced_by } => write!(
                formatter,
                "type `{}` has no definition; it is referenced by `{}`",
                missing.full(),
                referenced_by.full()
            ),
            Self::Cycle { at } => {
                write!(formatter, "type `{}` depends on itself through a cycle", at.full())
            }
            Self::NestingTooDeep { limit, at } => {
                write!(formatter, "type `{}` nests deeper than {limit} levels", at.full())
            }
            Self::TooLarge { limit, at } => {
                write!(
                    formatter,
                    "type `{}` expands to more than {limit} nested messages",
                    at.full()
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// One type waiting to have its own dependencies walked.
struct QueueItem {
    type_name: TypeName,
    dependencies: Vec<TypeName>,
}

/// The result of walking the dependency graph from a root type.
struct Walk {
    /// Every reachable type other than the root, in breadth-first order.
    order: Vec<TypeName>,
    /// Every reachable type's direct dependencies, root included.
    dependency_map: HashMap<TypeName, Vec<TypeName>>,
    /// Every type in `order`, mapped to its rendered body text.
    bodies: HashMap<TypeName, String>,
}

/// Walks the dependency graph from `root`, breadth-first, in field declaration order;
/// a type already seen is not visited again.
fn walk_from_root(
    specs: &SpecSet,
    root: &TypeName,
    root_spec: &MessageSpec,
) -> Result<Walk, ResolveError> {
    let mut order: Vec<TypeName> = Vec::new();
    let mut dependency_map: HashMap<TypeName, Vec<TypeName>> = HashMap::new();
    let mut bodies: HashMap<TypeName, String> = HashMap::new();
    let mut seen: HashSet<TypeName> = HashSet::new();
    let mut queue: VecDeque<QueueItem> = VecDeque::new();

    seen.insert(root.clone());
    let root_dependencies = root_spec.dependencies();
    dependency_map.insert(root.clone(), root_dependencies.clone());
    queue.push_back(QueueItem { type_name: root.clone(), dependencies: root_dependencies });

    while let Some(item) = queue.pop_front() {
        for dependency in &item.dependencies {
            let first_visit = seen.insert(dependency.clone());
            if !first_visit {
                continue;
            }

            let dependency_spec =
                specs.get(dependency).ok_or_else(|| ResolveError::MissingType {
                    missing: dependency.clone(),
                    referenced_by: item.type_name.clone(),
                })?;
            let dependency_dependencies = dependency_spec.dependencies();

            order.push(dependency.clone());
            bodies.insert(dependency.clone(), render_body(dependency_spec));
            dependency_map.insert(dependency.clone(), dependency_dependencies.clone());
            queue.push_back(QueueItem {
                type_name: dependency.clone(),
                dependencies: dependency_dependencies,
            });
        }
    }

    Ok(Walk { order, dependency_map, bodies })
}

struct Frame {
    type_name: TypeName,
    dependencies: Vec<TypeName>,
    next_index: usize,
}

impl Frame {
    fn enter(type_name: TypeName, dependency_map: &HashMap<TypeName, Vec<TypeName>>) -> Self {
        let dependencies = dependency_map.get(&type_name).cloned().unwrap_or_default();
        Self { type_name, dependencies, next_index: 0 }
    }
}

/// Searches from `root` for a type that depends on an ancestor on the current path,
/// and for a path deeper than [`MAX_NESTING_DEPTH`]. `dependency_map` must be complete,
/// as [`walk_from_root`] builds it.
///
/// A type reached again on a deeper path is expanded again, at most
/// [`MAX_NESTING_DEPTH`] times. An explicit stack replaces recursion.
fn detect_cycle(
    root: &TypeName,
    dependency_map: &HashMap<TypeName, Vec<TypeName>>,
) -> Result<(), ResolveError> {
    let mut on_path: HashSet<TypeName> = HashSet::new();
    let mut deepest_expansion: HashMap<TypeName, usize> = HashMap::new();
    let mut stack: Vec<Frame> = Vec::new();

    on_path.insert(root.clone());
    deepest_expansion.insert(root.clone(), ROOT_LEVEL);
    stack.push(Frame::enter(root.clone(), dependency_map));

    while let Some(frame) = stack.last_mut() {
        let next_child = frame.dependencies.get(frame.next_index).cloned();
        let Some(child) = next_child else {
            on_path.remove(&frame.type_name);
            stack.pop();
            continue;
        };
        frame.next_index = frame.next_index.saturating_add(1);

        let closes_a_cycle = on_path.contains(&child);
        if closes_a_cycle {
            return Err(ResolveError::Cycle { at: child });
        }

        // The stack is the path from the root, so the child sits one level below its top.
        let child_level = stack.len().saturating_add(1);
        let nests_too_deep = child_level > MAX_NESTING_DEPTH;
        if nests_too_deep {
            return Err(ResolveError::NestingTooDeep { limit: MAX_NESTING_DEPTH, at: child });
        }
        let expanded_at_least_as_deep =
            deepest_expansion.get(&child).is_some_and(|&level| level >= child_level);
        if expanded_at_least_as_deep {
            continue;
        }
        deepest_expansion.insert(child.clone(), child_level);
        on_path.insert(child.clone());
        stack.push(Frame::enter(child, dependency_map));
    }

    Ok(())
}

/// Renders one message's constants, then fields, as definition-text lines.
fn render_body(spec: &MessageSpec) -> String {
    let constants = spec.constants.iter().map(constant_line);
    let fields = spec.fields.iter().map(|field| format!("{} {}", field.render_type(), field.name));
    constants.chain(fields).collect::<Vec<_>>().join("\n")
}

/// One constant as a definition-text line, `type NAME=value`.
fn constant_line(constant: &Constant) -> String {
    format!("{} {}={}", constant.constant_type.render(), constant.name, constant.value)
}

/// The concatenated definition text for `root` and everything it reaches.
///
/// The root body comes first. Every type `root` reaches follows in breadth-first order,
/// once each, after an 80-character `=` line and a `MSG: package/Type` header.
///
/// # Errors
///
/// Returns [`ResolveError::MissingType`] when `root`, or a type it reaches,
/// names a type that has no definition in `specs`. Returns
/// [`ResolveError::Cycle`] when a type depends on itself, directly or
/// through other types.
pub fn definition_text(specs: &SpecSet, root: &TypeName) -> Result<String, ResolveError> {
    let (root_spec, walk) = checked_walk(specs, root)?;

    let separator = "=".repeat(SEPARATOR_WIDTH);
    let mut text = render_body(root_spec);
    text.extend(walk.order.iter().map(|type_name| {
        let body = walk.bodies.get(type_name).map_or("", String::as_str);
        format!("\n{separator}\nMSG: {}\n{body}", type_name.full())
    }));
    Ok(text)
}

/// Looks up `root`, walks its graph and refuses a cycle, a deep path or a
/// too-large expansion, in that order.
fn checked_walk<'a>(
    specs: &'a SpecSet,
    root: &TypeName,
) -> Result<(&'a MessageSpec, Walk), ResolveError> {
    let root_spec = specs.get(root).ok_or_else(|| ResolveError::MissingType {
        missing: root.clone(),
        referenced_by: root.clone(),
    })?;
    let walk = walk_from_root(specs, root, root_spec)?;
    detect_cycle(root, &walk.dependency_map)?;
    check_expanded_count(specs, root)?;
    Ok((root_spec, walk))
}

/// Checks that `root` and every type it reaches are defined in `specs`, that no type
/// depends on itself, that no path nests deeper than
/// [`MAX_NESTING_DEPTH`](crate::spec::MAX_NESTING_DEPTH), and that `root` expands to at
/// most [`MAX_EXPANDED_MESSAGES`](crate::spec::MAX_EXPANDED_MESSAGES) message instances.
///
/// A walker that does not resolve the definition first calls it once.
///
/// # Errors
///
/// Returns the error [`definition_text`] gives for the same set.
pub fn check_expansion(specs: &SpecSet, root: &TypeName) -> Result<(), ResolveError> {
    checked_walk(specs, root).map(|_| ())
}

/// Refuses a `root` that expands past [`MAX_EXPANDED_MESSAGES`].
fn check_expanded_count(specs: &SpecSet, root: &TypeName) -> Result<(), ResolveError> {
    if expanded_count(specs, root, &mut HashMap::new()) > MAX_EXPANDED_MESSAGES {
        return Err(ResolveError::TooLarge { limit: MAX_EXPANDED_MESSAGES, at: root.clone() });
    }
    Ok(())
}

/// The message instances of `root`: 1, plus for each message field its count times
/// the instances of its type. Memoised per type, saturating.
fn expanded_count(specs: &SpecSet, root: &TypeName, known: &mut HashMap<TypeName, u64>) -> u64 {
    if let Some(count) = known.get(root) {
        return *count;
    }
    let mut count: u64 = 1;
    if let Some(spec) = specs.get(root) {
        for field in &spec.fields {
            let FieldType::Message(nested) = &field.field_type else {
                continue;
            };
            let instances = match field.multiplicity {
                Multiplicity::Fixed(length) => u64::try_from(length).unwrap_or(u64::MAX),
                Multiplicity::Unit | Multiplicity::Variable => 1,
            };
            let nested_count = expanded_count(specs, nested, known);
            count = count.saturating_add(instances.saturating_mul(nested_count));
        }
    }
    known.insert(root.clone(), count);
    count
}

/// The identity a receiver checks: the MD5 of the root's constants and
/// fields, where each nested message field contributes its own identity.
///
/// # Errors
///
/// Returns any error [`definition_text`] can return.
pub fn message_identity(specs: &SpecSet, root: &TypeName) -> Result<String, ResolveError> {
    definition_text(specs, root)?;
    identity_of(specs, root, &mut HashMap::new())
}

/// The identities of one service, as ROS 1 computes them.
///
/// `request` and `response` are the [`message_identity`] of each half. `service` is the
/// MD5 of the request text followed by the response text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceIdentity {
    /// The service's own MD5.
    pub service: String,
    /// The request half's MD5.
    pub request: String,
    /// The response half's MD5.
    pub response: String,
}

/// The identities of a service: its own MD5 and the MD5 of each half.
///
///
/// # Errors
/// Returns a dependency-resolution error from either half of the service, or
/// [`ResolveError::NestingTooDeep`] when a half nests past the limit.
pub fn service_identity(
    specs: &SpecSet,
    service: &ServiceSpec,
) -> Result<ServiceIdentity, ResolveError> {
    let mut identities = HashMap::new();
    let request = half_identity(specs, &service.request.type_name, &mut identities)?;
    let response = half_identity(specs, &service.response.type_name, &mut identities)?;
    let mut text = message_identity_text(&service.request, &identities)?;
    text.push_str(&message_identity_text(&service.response, &identities)?);
    Ok(ServiceIdentity { service: md5_hex(text.as_bytes()), request, response })
}

/// The MD5 of one service half, checked as a root first. Its nested identities are
/// left in `identities`.
fn half_identity(
    specs: &SpecSet,
    half: &TypeName,
    identities: &mut HashMap<TypeName, String>,
) -> Result<String, ResolveError> {
    definition_text(specs, half)?;
    identity_of(specs, half, identities)
}

/// The identity of `root`, with the identity of every type below it added to
/// `identities`. Each type is hashed once. The caller has run [`definition_text`] on `root`.
fn identity_of(
    specs: &SpecSet,
    root: &TypeName,
    identities: &mut HashMap<TypeName, String>,
) -> Result<String, ResolveError> {
    let type_count = specs.values().count();
    let mut stack = vec![root.clone()];
    while let Some(top) = stack.last().cloned() {
        if identities.contains_key(&top) {
            stack.pop();
            continue;
        }
        if stack.len() > type_count {
            return Err(ResolveError::Cycle { at: top });
        }
        let Some(spec) = specs.get(&top) else {
            return Err(ResolveError::MissingType { missing: top.clone(), referenced_by: top });
        };
        if let Some(child) =
            spec.dependencies().into_iter().find(|name| !identities.contains_key(name))
        {
            stack.push(child);
            continue;
        }
        let identity = md5_hex(message_identity_text(spec, identities)?.as_bytes());
        identities.insert(top, identity);
        stack.pop();
    }
    identities.get(root).cloned().ok_or_else(|| ResolveError::MissingType {
        missing: root.clone(),
        referenced_by: root.clone(),
    })
}

/// The text one message contributes to an identity: constant lines, then field lines.
/// A nested message field is written as its identity and the field name, no brackets.
fn message_identity_text(
    spec: &MessageSpec,
    identities: &HashMap<TypeName, String>,
) -> Result<String, ResolveError> {
    let mut lines: Vec<String> = spec.constants.iter().map(constant_line).collect();
    for field in &spec.fields {
        let line = match &field.field_type {
            FieldType::Message(name) => {
                let identity = identities.get(name).ok_or_else(|| ResolveError::MissingType {
                    missing: name.clone(),
                    referenced_by: spec.type_name.clone(),
                })?;
                format!("{identity} {}", field.name)
            }
            builtin => {
                format!("{}{} {}", builtin.render(), field.multiplicity.render(), field.name)
            }
        };
        lines.push(line);
    }
    Ok(lines.join("\n"))
}
