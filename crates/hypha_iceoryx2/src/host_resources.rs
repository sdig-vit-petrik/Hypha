//! The iceoryx2 resources of every domain on this host, for `hypha clean`: the domain
//! each one belongs to, the liveness of a domain's nodes, and the removal of one.
//!
//! A resource is named after its domain's prefix: `iox2-<domain>-` (`domain.rs`), or
//! `iox2_` for the domain "none". It is a file, directory or socket under the iceoryx2
//! root (`/tmp/iceoryx2`), which keeps the services, the node monitors and hypha's schema
//! directories, or a shared-memory object: a file of `/dev/shm` on Linux, a kernel object
//! that the state file `/tmp/<name>.shm_state` names on macOS. The per-node directories
//! `nodes/<id>/` carry no domain in their name and hold the tags of their node's
//! domain, so names are matched inside them, never whole directories.
//!
//! iceoryx2 tells whether a node lives: its process holds a lock on the node's monitor,
//! which the kernel releases when the process ends. It also removes the resources of a
//! dead node it can read (`DeadNodeView::blocking_remove_stale_resources`), but not those
//! of a node it reports `Inaccessible` or `Undefined`, nor the domain's global management
//! segment, nor hypha's schema directories and lock. A domain nobody uses is therefore
//! removed by name, resource by resource.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use iceoryx2::node::{Node, NodeState, NodeView as _};
use iceoryx2::prelude::{CallbackProgression, FileName, SemanticString as _};
use iceoryx2_bb_posix::shared_memory::SharedMemory;

use crate::transport::Ipc;

/// The resource prefix of the domain "none": iceoryx2's own.
const NO_DOMAIN_PREFIX: &str = "iox2_";

/// One resource of a domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// A file, directory or socket under the iceoryx2 root.
    Path(PathBuf),
    /// A shared-memory object, by its name.
    SharedMemory(String),
}

impl std::fmt::Display for Resource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(path) => write!(formatter, "{}", path.display()),
            Self::SharedMemory(name) => write!(formatter, "shared memory {name}"),
        }
    }
}

/// What iceoryx2 knows of one node of a domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeLiveness {
    /// Its process lives. The name and the process id when the node's files tell them.
    Alive {
        /// The node's name, empty when its details cannot be read.
        name: String,
        /// The process id, `None` when it cannot be read.
        pid: Option<i32>,
    },
    /// Its process ended without removing the node.
    Dead,
    /// This user may not read whether it lives: it is another user's.
    Inaccessible,
    /// Its files are inconsistent, so whether it lives is unknown.
    Undefined,
}

/// The domain whose resource is named `name`: `Some("")` for the domain "none", `None`
/// for a name of no domain.
#[must_use]
pub fn domain_of_name(name: &str) -> Option<&str> {
    if let Some(rest) = name.strip_prefix("iox2-") {
        let (domain, _) = rest.split_once('-')?;
        return crate::domain::is_domain_name(domain).then_some(domain);
    }
    name.starts_with(NO_DOMAIN_PREFIX).then_some("")
}

/// The iceoryx2 root, which every domain shares.
fn root() -> PathBuf {
    PathBuf::from(iceoryx2::config::Config::default().global.root_path().as_str())
}

/// Every resource of every domain on this host, each with its domain.
///
/// # Errors
/// The message of a directory that cannot be read.
pub fn host_resources() -> Result<Vec<(String, Resource)>, String> {
    let mut found = Vec::new();
    collect_paths(&root(), &mut found)?;
    for name in SharedMemory::list() {
        let Ok(name) = std::str::from_utf8(name.as_bytes()) else {
            continue; // not a name iceoryx2 gives
        };
        if let Some(domain) = domain_of_name(name) {
            found.push((domain.to_owned(), Resource::SharedMemory(name.to_owned())));
        }
    }
    Ok(found)
}

/// Adds the resources under `directory`: a named entry whole, and the entries of a
/// directory that is named after no domain.
fn collect_paths(directory: &Path, found: &mut Vec<(String, Resource)>) -> Result<(), String> {
    let cannot_read =
        |cause: std::io::Error| format!("cannot read {}: {cause}", directory.display());
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(cause) if cause.kind() == ErrorKind::NotFound => return Ok(()),
        Err(cause) => return Err(cannot_read(cause)),
    };
    for entry in entries {
        let entry = entry.map_err(cannot_read)?;
        let path = entry.path();
        if let Some(domain) = entry.file_name().to_str().and_then(domain_of_name) {
            found.push((domain.to_owned(), Resource::Path(path)));
        } else if entry.file_type().map_err(cannot_read)?.is_dir() {
            collect_paths(&path, found)?;
        }
    }
    Ok(())
}

/// The nodes of `domain` (empty for the domain "none") and whether each lives.
///
/// # Errors
/// The message of a node list iceoryx2 cannot read.
pub fn domain_nodes(domain: &str) -> Result<Vec<NodeLiveness>, String> {
    let config = crate::domain::config_of(domain)?;
    let mut nodes = Vec::new();
    Node::<Ipc>::list(&config, |state| {
        nodes.push(match state {
            NodeState::Alive(view) => NodeLiveness::Alive {
                name: view
                    .details()
                    .as_ref()
                    .map_or_else(String::new, |details| details.name().as_str().to_owned()),
                pid: view.id().pid::<Ipc>().ok().map(|pid| pid.value()),
            },
            NodeState::Dead(_) => NodeLiveness::Dead,
            NodeState::Inaccessible(_) => NodeLiveness::Inaccessible,
            NodeState::Undefined(_) => NodeLiveness::Undefined,
        });
        CallbackProgression::Continue
    })
    .map_err(|cause| format!("cannot list the nodes of the domain: {cause:?}"))?;
    Ok(nodes)
}

/// Removes `resource`, and the node directory it leaves empty. A resource another
/// process removed first counts as removed.
///
/// # Errors
/// The message of a removal the operating system refuses.
pub fn remove_resource(resource: &Resource) -> Result<(), String> {
    let refused = |cause: &dyn std::fmt::Display| format!("cannot remove {resource}: {cause}");
    match resource {
        Resource::SharedMemory(name) => {
            let name =
                FileName::new(name.as_bytes()).map_err(|cause| refused(&format!("{cause:?}")))?;
            SharedMemory::remove(&name).map_err(|cause| refused(&format!("{cause:?}")))?;
        }
        Resource::Path(path) => {
            let removed = match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
                Ok(_) => std::fs::remove_file(path),
                Err(cause) => Err(cause),
            };
            match removed {
                Err(cause) if cause.kind() != ErrorKind::NotFound => return Err(refused(&cause)),
                _ => {}
            }
            remove_emptied_node_directory(path).map_err(|cause| refused(&cause))?;
        }
    }
    Ok(())
}

/// Removes the node directory `nodes/<id>/` that held `path` once it is empty; one that
/// still holds another domain's files stays.
fn remove_emptied_node_directory(path: &Path) -> std::io::Result<()> {
    let Some(directory) = path.parent() else {
        return Ok(());
    };
    if directory.parent() != Some(root().join("nodes").as_path()) {
        return Ok(());
    }
    match std::fs::remove_dir(directory) {
        Err(cause)
            if !matches!(cause.kind(), ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty) =>
        {
            Err(cause)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::domain_of_name;

    #[test]
    fn a_name_belongs_to_the_domain_its_prefix_names() {
        assert_eq!(domain_of_name("iox2-t_12_3-abc.event"), Some("t_12_3"));
        assert_eq!(domain_of_name("iox2-t_12_3-_hypha_schemas"), Some("t_12_3"));
        assert_eq!(domain_of_name("iox2_1029966763.event"), Some(""));
        assert_eq!(domain_of_name("iox2__hypha_schemas"), Some(""));
    }

    #[test]
    fn a_name_of_no_domain_belongs_to_none() {
        for name in ["nodes", "services", "1029967236537346", "iox2-Bad-x", "iox2-t_1", "iox2"] {
            assert_eq!(domain_of_name(name), None, "{name}");
        }
    }
}
