//! ROS 1 message definitions read from `.msg` files, for tools that work offline.
//!
//! A message path is a directory of packages: `<path>/<package>/<Name>.msg`, as hypha
//! installs them in `share/hypha/msg`, or `<path>/<package>/msg/<Name>.msg`, as a ROS
//! package keeps them. A type is looked up in the paths in order; the first file wins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hypha_node::sdk::msgdef::parse::parse_message;
use hypha_node::sdk::msgdef::resolve::{SpecSet, definition_text, message_identity};
use hypha_node::sdk::msgdef::spec::TypeName;

/// What a recording needs of one type: its full ROS 1 definition and its MD5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// `package/Name`.
    pub type_name: String,
    /// The definition with every type it names, as `rosmsg` prints it.
    pub definition: String,
    /// The ROS 1 MD5 of the type.
    pub md5: String,
}

/// The message definitions of a set of message paths, loaded as they are asked for.
#[derive(Debug, Default)]
pub struct MessageLibrary {
    paths: Vec<PathBuf>,
    specs: SpecSet,
    resolved: BTreeMap<String, Resolved>,
}

/// The ROS 1 name of a ROS 2 message type: `sensor_msgs/msg/Imu` is `sensor_msgs/Imu`.
/// `None` for a service, an action or a name of another form.
#[must_use]
pub fn ros1_type_of(ros2_type: &str) -> Option<TypeName> {
    let mut parts = ros2_type.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(package), Some("msg"), Some(name), None)
            if !package.is_empty() && !name.is_empty() =>
        {
            Some(TypeName::new(package, name))
        }
        _ => None,
    }
}

impl MessageLibrary {
    /// A library that looks types up in `paths`, in order.
    #[must_use]
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths, specs: SpecSet::new(), resolved: BTreeMap::new() }
    }

    /// The parsed definitions loaded so far: those of every type resolved and every type
    /// they name.
    #[must_use]
    pub fn specs(&self) -> &SpecSet {
        &self.specs
    }

    /// The definition and MD5 of `type_name`, loading it and every type it names.
    ///
    /// # Errors
    /// Text that names the type that has no file in any path, a file that does not
    /// parse, or a definition that does not resolve.
    pub fn resolve(&mut self, type_name: &TypeName) -> Result<Resolved, String> {
        let full = type_name.full();
        if let Some(resolved) = self.resolved.get(&full) {
            return Ok(resolved.clone());
        }
        let mut pending = vec![type_name.clone()];
        while let Some(next) = pending.pop() {
            if self.specs.get(&next).is_some() {
                continue;
            }
            let path = self
                .find(&next)
                .ok_or_else(|| format!("no definition of {} in the message paths", next.full()))?;
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let spec = parse_message(next.clone(), &text)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            pending.extend(spec.dependencies());
            self.specs.insert(spec);
        }
        let definition =
            definition_text(&self.specs, type_name).map_err(|error| format!("{full}: {error}"))?;
        let md5 =
            message_identity(&self.specs, type_name).map_err(|error| format!("{full}: {error}"))?;
        let resolved = Resolved { type_name: full.clone(), definition, md5 };
        self.resolved.insert(full, resolved.clone());
        Ok(resolved)
    }

    /// The file of `type_name` in the first path that has one.
    fn find(&self, type_name: &TypeName) -> Option<PathBuf> {
        let file = format!("{}.msg", type_name.name);
        self.paths.iter().find_map(|path| {
            let package = path.join(&type_name.package);
            [package.join(&file), package.join("msg").join(&file)]
                .into_iter()
                .find(|candidate| candidate.is_file())
        })
    }
}

/// The message path of an installed SDK, beside the tool: `<prefix>/share/hypha/msg`
/// when the tool is `<prefix>/bin/hypha`. `None` when there is none.
#[must_use]
pub fn installed_message_path(executable: &Path) -> Option<PathBuf> {
    let prefix = executable.parent()?.parent()?;
    let path = prefix.join("share").join("hypha").join("msg");
    path.is_dir().then_some(path)
}
