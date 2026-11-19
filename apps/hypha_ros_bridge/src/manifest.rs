//! TOML manifest parsing and startup validation.

use std::path::Path;

use serde::Deserialize;

use crate::error::BridgeError;

/// Direction of one bridge route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeDirection {
    /// Receive from ROS 1 and publish to hypha.
    Ros1ToHypha,
    /// Receive from hypha and publish to ROS 1.
    HyphaToRos1,
    /// Enable both directions.
    Bidirectional,
}

impl BridgeDirection {
    /// Returns whether this direction includes the requested flow.
    #[must_use]
    pub const fn allows(self, flow: crate::routing::BridgeFlow) -> bool {
        matches!(
            (self, flow),
            (Self::Ros1ToHypha, crate::routing::BridgeFlow::Ros1ToHypha)
                | (Self::HyphaToRos1, crate::routing::BridgeFlow::HyphaToRos1)
                | (Self::Bidirectional, _)
        )
    }
}

/// One manifest route. The bridge never parses `ros_type`, `ros_md5`, or
/// `ros_definition` as a schema: they label the opaque bytes it moves.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeRoute {
    /// Local hypha topic.
    pub hypha_topic: String,
    /// ROS 1 topic.
    pub ros_topic: String,
    /// Optional ROS 1 message type, such as `std_msgs/String`.
    #[serde(default)]
    pub ros_type: Option<String>,
    /// Optional canonical ROS 1 MD5 value.
    #[serde(default)]
    pub ros_md5: Option<String>,
    /// Optional expanded ROS 1 message definition, as `gendeps --cat` would print it.
    /// See <https://wiki.ros.org/roslib/gentools>.
    #[serde(default)]
    pub ros_definition: Option<String>,
    /// Enabled direction or directions.
    pub direction: BridgeDirection,
    /// Whether ROS 1 should retain and replay the last message on this
    /// route's topic to a late subscriber.
    #[serde(default)]
    pub latched: bool,
}

const OPAQUE_ROS_TYPE: &str = "ros1/opaque";
const OPAQUE_ROS_MD5: &str = "00000000000000000000000000000000";

impl BridgeRoute {
    /// Returns whether all three ROS identity fields are present.
    #[must_use]
    pub fn has_ros_identity(&self) -> bool {
        self.ros_type.is_some() && self.ros_md5.is_some() && self.ros_definition.is_some()
    }

    /// Returns whether some, but not all, of the three ROS identity fields
    /// are present. Validation rejects this.
    #[must_use]
    pub fn has_partial_ros_identity(&self) -> bool {
        let fields =
            [self.ros_type.is_some(), self.ros_md5.is_some(), self.ros_definition.is_some()];
        fields.contains(&true) && fields.contains(&false)
    }

    /// Returns the ROS type used for the hypha schema identity.
    #[must_use]
    pub fn effective_ros_type(&self) -> &str {
        self.declared_identity(self.ros_type.as_deref(), OPAQUE_ROS_TYPE)
    }

    /// Returns the MD5 used for the hypha schema identity.
    #[must_use]
    pub fn effective_ros_md5(&self) -> &str {
        self.declared_identity(self.ros_md5.as_deref(), OPAQUE_ROS_MD5)
    }

    /// Returns the expanded definition used for the ROS connection header.
    #[must_use]
    pub fn effective_ros_definition(&self) -> &str {
        self.declared_identity(self.ros_definition.as_deref(), "")
    }

    /// The declared identity field when all three are present, else `opaque`.
    fn declared_identity<'a>(&'a self, declared: Option<&'a str>, opaque: &'a str) -> &'a str {
        if self.has_ros_identity() { declared.unwrap_or(opaque) } else { opaque }
    }
}

/// The complete bridge manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeManifest {
    /// All routes enabled for this process.
    pub routes: Vec<BridgeRoute>,
}

impl BridgeManifest {
    /// Reads and validates one TOML manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, parsed, or validated.
    pub fn load(path: &Path) -> Result<Self, BridgeError> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            BridgeError::Config(format!("cannot read '{}': {error}", path.display()))
        })?;
        Self::parse(&text).map_err(|error| {
            BridgeError::Config(format!("cannot parse '{}': {error}", path.display()))
        })
    }

    /// Parses and validates a TOML manifest string.
    ///
    /// # Errors
    ///
    /// Returns an error when the text cannot be parsed or a route is invalid.
    pub fn parse(text: &str) -> Result<Self, BridgeError> {
        let manifest = toml::from_str::<Self>(text)
            .map_err(|error| BridgeError::Config(format!("cannot parse manifest: {error}")))?;
        manifest.validate()
    }

    /// Validates all routes without opening a network connection.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest is empty or a route is invalid.
    pub fn validate(mut self) -> Result<Self, BridgeError> {
        if self.routes.is_empty() {
            return Err(BridgeError::Config("manifest must contain at least one route".into()));
        }

        for route in &mut self.routes {
            if let Some(ros_md5) = route.ros_md5.as_mut() {
                ros_md5.make_ascii_lowercase();
            }
            validate_route(route)?;
        }
        Ok(self)
    }
}

/// A configuration error that names the route it concerns.
fn route_error(route: &BridgeRoute, problem: &str) -> BridgeError {
    BridgeError::Config(format!("route '{}' {problem}", route.ros_topic))
}

/// A configuration error for a `ros_definition` that cannot be parsed.
fn unparseable_definition(route: &BridgeRoute, reason: &str) -> BridgeError {
    route_error(route, &format!("has an unparseable ros_definition: {reason}"))
}

fn validate_route(route: &BridgeRoute) -> Result<(), BridgeError> {
    if route.has_partial_ros_identity() {
        return Err(route_error(
            route,
            "gives only some of ros_type, ros_md5, and ros_definition: give all three for a \
             real identity, or omit all three for an opaque route",
        ));
    }
    let has_identity = route.has_ros_identity();
    if route.direction.allows(crate::routing::BridgeFlow::HyphaToRos1) && !has_identity {
        return Err(BridgeError::Config(
            "ros_type, ros_md5, and ros_definition are required for hypha_to_ros1 routes".into(),
        ));
    }
    let has_valid_topics = !route.hypha_topic.is_empty() && !route.ros_topic.is_empty();
    if !has_valid_topics {
        return Err(BridgeError::Config("route topics must not be empty".into()));
    }
    if !has_identity {
        return Ok(());
    }

    let Some(ros_md5) = route.ros_md5.as_deref() else {
        return Ok(());
    };
    if !hypha_node::sdk::is_valid_md5(ros_md5) {
        return Err(route_error(
            route,
            "has a malformed ros_md5: expected 32 lower-case hexadecimal characters",
        ));
    }

    let Some(ros_definition) = route.ros_definition.as_deref() else {
        return Ok(());
    };
    if ros_definition.is_empty() {
        return Err(route_error(route, "has an empty ros_definition"));
    }
    validate_definition_sections(route)?;
    let Some(ros_type) = route.ros_type.as_deref() else {
        return Ok(());
    };
    let computed_md5 = roslibrust::md5sum::from_message_definition(ros_type, ros_definition)
        .map_err(|error| unparseable_definition(route, &error.to_string()))?;
    if !ros_md5.eq_ignore_ascii_case(&computed_md5) {
        return Err(route_error(
            route,
            &format!(
                "has a ros_md5 that does not match its ros_definition: declared {ros_md5}, \
                 computed {computed_md5}"
            ),
        ));
    }
    Ok(())
}

fn validate_definition_sections(route: &BridgeRoute) -> Result<(), BridgeError> {
    const SECTION_SEPARATOR: &str =
        "================================================================================\n";
    let Some(definition) = route.ros_definition.as_deref() else {
        return Ok(());
    };

    for section in definition.split(SECTION_SEPARATOR).skip(1) {
        let Some(line) = section.lines().next() else {
            return Err(unparseable_definition(route, "empty dependency section"));
        };
        let mut fields = line.split_whitespace();
        let marker = fields.next();
        let message_type = fields.next();
        let has_extra_field = fields.next().is_some();
        let valid_header = marker == Some("MSG:") && message_type.is_some() && !has_extra_field;
        if !valid_header {
            return Err(unparseable_definition(
                route,
                "dependency section must start with 'MSG: <type>'",
            ));
        }
    }
    Ok(())
}
