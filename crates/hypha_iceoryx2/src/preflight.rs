//! Checks of the domain's shared files, run once per process before it first
//! uses the domain. iceoryx2 reports a foreign owner, or a container without the
//! shared mount, only as an internal error or as silence.

use std::path::{Path, PathBuf};

use iceoryx2::config::Config;
use iceoryx2::prelude::SemanticString as _;

/// The soft open-file limit a process asks for: macOS's `OPEN_MAX`.
const OPEN_FILES_WANTED: u64 = 10_240;

/// The soft open-file limit to set, or `None` to keep `current`; never lowered.
pub(crate) fn wanted_open_files(current: Option<u64>, maximum: Option<u64>) -> Option<u64> {
    let target = maximum.map_or(OPEN_FILES_WANTED, |max| max.min(OPEN_FILES_WANTED));
    current.filter(|&soft| soft < target).map(|_| target)
}

/// Raise the soft open-file limit towards the hard limit.
///
/// A node with a few dozen ports passes macOS's 256, and then every service
/// creation fails with an unnamed iceoryx2 `InternalFailure`.
pub(crate) fn raise_open_file_limit() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let limit = getrlimit(Resource::Nofile);
    let Some(target) = wanted_open_files(limit.current, limit.maximum) else {
        return;
    };
    let soft = limit.current.unwrap_or_default();
    if let Err(cause) = setrlimit(Resource::Nofile, Rlimit { current: Some(target), ..limit }) {
        log::warn!(target: "hypha/transport", "hypha: cannot raise the open-file limit from {soft} to {target}: {cause}; a node with many ports can fail with InternalFailure (raise it with ulimit -n)");
    }
}

/// The first entry whose owner is not `me`, with that owner.
pub(crate) fn foreign_owner(entries: &[(PathBuf, u32)], me: u32) -> Option<(PathBuf, u32)> {
    entries.iter().find(|(_, owner)| *owner != me).cloned()
}

/// True when a root entry named `name` is the domain's own: its prefix, or `services`
/// or `nodes`. Another domain's entry must not decide this domain's check.
pub(crate) fn is_domain_root_entry(
    name: &str,
    prefix: &str,
    shared_directories: &[String],
) -> bool {
    let own_prefix = name.starts_with(prefix);
    let shared = shared_directories.iter().any(|directory| directory == name);
    own_prefix || shared
}

/// Refuse a domain whose shared files another user owns.
///
///
/// # Errors
/// The message to log when a domain entry belongs to another user or cannot be read.
pub(crate) fn check(domain: &str, config: &Config) -> Result<(), String> {
    let root =
        PathBuf::from(String::from_utf8_lossy(config.global.root_path().as_bytes()).as_ref());
    let prefix = String::from_utf8_lossy(config.global.prefix.as_bytes()).into_owned();
    let shared_directories = [
        String::from_utf8_lossy(config.global.service.directory.as_bytes()).into_owned(),
        String::from_utf8_lossy(config.global.node.directory.as_bytes()).into_owned(),
    ];
    let mut entries = Vec::new();
    owned_entries(
        &root,
        |name| is_domain_root_entry(name, &prefix, &shared_directories),
        &mut entries,
    )?;
    #[cfg(target_os = "linux")]
    owned_entries(Path::new("/dev/shm"), |name| name.starts_with(&prefix), &mut entries)?;
    let me = rustix::process::geteuid().as_raw();
    let domain = if domain.is_empty() { "<default>" } else { domain };
    let Some((path, owner)) = foreign_owner(&entries, me) else {
        #[cfg(target_os = "linux")]
        {
            let marker = format!("{prefix}{MOUNT_MARKER}");
            return check_markers(&root.join(&marker), &Path::new("/dev/shm").join(&marker));
        }
        #[cfg(not(target_os = "linux"))]
        return Ok(());
    };
    Err(format!(
        "hypha_iceoryx2: domain '{domain}' belongs to uid {owner} ({}), this process runs as uid {me}; \
         every hypha process of a domain must use the same numeric UID/GID (containers: set the same \
         'user:' everywhere)",
        path.display()
    ))
}

/// The file name, after the domain prefix, of the mount marker.
#[cfg(target_os = "linux")]
const MOUNT_MARKER: &str = "hypha_mount_id";

/// Refuse when the marker in the iceoryx2 root directory, `meta`, and the one in
/// `/dev/shm`, `shm`, differ. The first process of a domain writes both.
// Only Linux calls it; the unit tests run it everywhere.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn check_markers(meta: &Path, shm: &Path) -> Result<(), String> {
    use std::hash::{BuildHasher as _, Hasher as _};

    if let Some(dir) = meta.parent() {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o750)
            .create(dir)
            .map_err(|cause| format!("hypha_iceoryx2: cannot create {}: {cause}", dir.display()))?;
    }
    let random = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    let id = create_or_read(meta, &format!("{:016x}{:016x}", random(), random()))?;
    let seen = create_or_read(shm, &id)?;
    if seen == id {
        return Ok(());
    }
    let root = meta.parent().unwrap_or(meta);
    Err(format!(
        "hypha_iceoryx2: /dev/shm and {} are not shared by the same processes (markers {id} vs {seen}). \
         In containers, mount the same volumes at /dev/shm and /tmp/iceoryx2 in every container; on a \
         host, run ./clean-shm.sh if only one of them was cleaned.",
        root.display()
    ))
}

/// The content of `path`, written first as `content` when it does not exist.
/// Written under a temporary name and hard-linked, so a reader never sees half of
/// it. An existing marker is only read: a full `/dev/shm` must not stop the node.
#[cfg(any(target_os = "linux", test))]
fn create_or_read(path: &Path, content: &str) -> Result<String, String> {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let failed = |what: &str, cause: std::io::Error| {
        format!("hypha_iceoryx2: cannot {what} the mount marker {}: {cause}", path.display())
    };
    match std::fs::read_to_string(path) {
        Ok(existing) => return Ok(existing),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(failed("read", cause)),
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.{nanos}.{write}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    if let Err(cause) = std::fs::write(&temporary, content) {
        // A write that ran out of space can leave an empty file behind.
        let _ = std::fs::remove_file(&temporary);
        return Err(failed("write", cause));
    }
    let linked = std::fs::hard_link(&temporary, path);
    std::fs::remove_file(&temporary)
        .map_err(|cause| failed("remove the temporary file of", cause))?;
    match linked {
        Ok(()) => Ok(content.to_owned()),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::read_to_string(path).map_err(|cause| failed("read", cause))
        }
        Err(cause) => Err(failed("create", cause)),
    }
}

/// Push the entries of `dir` whose file name passes `wanted`, with their owner.
fn owned_entries(
    dir: &Path,
    wanted: impl Fn(&str) -> bool,
    out: &mut Vec<(PathBuf, u32)>,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;

    let unreadable =
        |cause: std::io::Error| format!("hypha_iceoryx2: cannot read {}: {cause}", dir.display());
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(cause) => return Err(unreadable(cause)),
    };
    for entry in listing {
        let entry = entry.map_err(unreadable)?;
        if !entry.file_name().to_str().is_some_and(&wanted) {
            continue;
        }
        // An entry removed since the listing is no longer shared.
        match entry.metadata() {
            Ok(metadata) => out.push((entry.path(), metadata.uid())),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(unreadable(cause)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_markers, foreign_owner, is_domain_root_entry, wanted_open_files};
    use std::path::PathBuf;

    #[test]
    fn the_open_file_limit_is_raised_to_the_hard_limit_at_most_open_max() {
        assert_eq!(wanted_open_files(Some(256), None), Some(10_240));
        assert_eq!(wanted_open_files(Some(256), Some(4096)), Some(4096));
        assert_eq!(wanted_open_files(Some(256), Some(256)), None);
        assert_eq!(wanted_open_files(Some(20_000), None), None);
        assert_eq!(wanted_open_files(None, None), None);
    }

    fn fresh_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos());
        std::env::temp_dir().join(format!("hypha_marker_{label}_{}_{nanos}", std::process::id()))
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn the_first_check_writes_equal_markers_and_a_different_one_is_refused() {
        let dir = fresh_dir("first");
        let (meta, shm) = (dir.join("root/marker"), dir.join("shm_marker"));
        assert_eq!(check_markers(&meta, &shm), Ok(()));
        let id = std::fs::read_to_string(&meta).expect("meta marker");
        assert_eq!(id.len(), 32);
        assert_eq!(std::fs::read_to_string(&shm).expect("shm marker"), id);
        assert_eq!(check_markers(&meta, &shm), Ok(()));

        std::fs::write(&shm, "another").expect("overwrite");
        let refused = check_markers(&meta, &shm).expect_err("markers differ");
        assert!(refused.contains(&format!("markers {id} vs another")), "{refused}");
        assert_eq!(
            std::fs::read_dir(dir.join("root")).expect("root").count(),
            1,
            "no temporary file stays"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn existing_markers_are_read_without_writing() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = fresh_dir("readonly");
        let (meta, shm) = (dir.join("root/marker"), dir.join("shm/marker"));
        std::fs::create_dir_all(dir.join("shm")).expect("shm dir");
        assert_eq!(check_markers(&meta, &shm), Ok(()));
        // A directory that takes no new file, as a full /dev/shm does.
        for sub in ["root", "shm"] {
            let permissions = std::fs::Permissions::from_mode(0o500);
            std::fs::set_permissions(dir.join(sub), permissions).expect("read-only");
        }
        let checked = check_markers(&meta, &shm);
        for sub in ["root", "shm"] {
            let permissions = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(dir.join(sub), permissions).expect("writable");
        }
        assert_eq!(checked, Ok(()));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn racing_first_checks_agree_on_one_marker() {
        let dir = fresh_dir("race");
        let (meta, shm) = (dir.join("root/marker"), dir.join("shm_marker"));
        std::fs::create_dir_all(&dir).expect("dir");
        let results: Vec<_> = std::thread::scope(|scope| {
            let workers: Vec<_> =
                (0..8).map(|_| scope.spawn(|| check_markers(&meta, &shm))).collect();
            workers.into_iter().map(|worker| worker.join().expect("worker")).collect()
        });
        assert!(results.iter().all(Result::is_ok), "{results:?}");
        assert_eq!(
            std::fs::read_to_string(&meta).expect("meta"),
            std::fs::read_to_string(&shm).expect("shm")
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_root_scan_takes_this_domains_entries_and_the_shared_directories_only() {
        let prefix = "iox2-mine-";
        let shared = ["services".to_owned(), "nodes".to_owned()];
        assert!(is_domain_root_entry("iox2-mine-hypha_mount_id", prefix, &shared));
        assert!(is_domain_root_entry("iox2-mine-hypha_schemas.lock", prefix, &shared));
        assert!(is_domain_root_entry("services", prefix, &shared));
        assert!(is_domain_root_entry("nodes", prefix, &shared));
        assert!(!is_domain_root_entry("iox2-other-hypha_mount_id", prefix, &shared));
        assert!(!is_domain_root_entry("iox2_hypha_mount_id", prefix, &shared));
        assert!(!is_domain_root_entry("hypha_schemas.lock", prefix, &shared));
    }

    #[test]
    fn names_the_first_entry_of_another_owner() {
        let entries =
            [(PathBuf::from("/a"), 1000), (PathBuf::from("/b"), 2000), (PathBuf::from("/c"), 0)];
        assert_eq!(foreign_owner(&entries, 1000), Some((PathBuf::from("/b"), 2000)));
        assert_eq!(foreign_owner(&entries[..1], 1000), None);
        assert_eq!(foreign_owner(&[], 1000), None);
    }
}
