//! A read-only memory map of a file.

use std::fs::File;
use std::ops::Deref;
use std::os::fd::AsRawFd as _;
use std::ptr::NonNull;

/// The bytes of a file, mapped read-only. Reading them reads the file.
#[derive(Debug)]
pub struct ReadOnlyMap {
    /// The mapping, or `None` for an empty file, which has nothing to map.
    start: Option<NonNull<u8>>,
    len: usize,
}

// SAFETY: the map is read-only memory that this value alone owns and unmaps; nothing
// writes through it, so any thread may hold it.
unsafe impl Send for ReadOnlyMap {}
// SAFETY: as above: shared references only read.
unsafe impl Sync for ReadOnlyMap {}

impl Deref for ReadOnlyMap {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        let Some(start) = self.start else {
            return &[];
        };
        // SAFETY: `start` is a live read-only mapping of `len` bytes until the drop, and
        // `map_read_only` states the condition under which its bytes do not change.
        unsafe { std::slice::from_raw_parts(start.as_ptr(), self.len) }
    }
}

impl Drop for ReadOnlyMap {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            // SAFETY: `start` and `len` are the mapping `mmap` returned, unmapped once;
            // no reference to its bytes outlives this value. A failed unmap leaves the
            // mapping in place, which is not unsound.
            unsafe { libc::munmap(start.as_ptr().cast(), self.len) };
        }
    }
}

/// Map all of `file` read-only, for data that can be larger than memory.
///
/// Sound only while no process changes the file for as long as the map lives: a write
/// changes borrowed bytes, and a truncation makes a read end the process with SIGBUS.
/// Its one user, the player of `hypha_tools`, maps only complete recordings.
///
/// # Errors
/// The error of reading the file's size or of `mmap`; `FileTooLarge` for a file whose
/// size no address range holds.
pub fn map_read_only(file: &File) -> std::io::Result<ReadOnlyMap> {
    let len = usize::try_from(file.metadata()?.len())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::FileTooLarge))?;
    if len == 0 {
        return Ok(ReadOnlyMap { start: None, len });
    }
    // SAFETY: a new read-only mapping of `len` bytes of an open file, at an address the
    // system chooses; only a complete recording is mapped, and nothing writes it meanwhile.
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if address == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ReadOnlyMap { start: NonNull::new(address.cast()), len })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a failed setup is a test failure")]
mod tests {
    use std::io::Write as _;

    use super::map_read_only;

    fn file_with(bytes: &[u8], label: &str) -> (std::path::PathBuf, std::fs::File) {
        let path =
            std::env::temp_dir().join(format!("hypha_sys_map_{label}_{}", std::process::id()));
        std::fs::File::create(&path).expect("create").write_all(bytes).expect("write");
        let file = std::fs::File::open(&path).expect("open");
        (path, file)
    }

    #[test]
    fn a_map_reads_the_bytes_of_its_file() {
        let bytes: Vec<u8> = (0..=u8::MAX).cycle().take(70_000).collect();
        let (path, file) = file_with(&bytes, "bytes");
        let map = map_read_only(&file).expect("map");
        assert_eq!(&*map, bytes.as_slice());
        drop(map);
        std::fs::remove_file(path).expect("remove");
    }

    #[test]
    fn an_empty_file_maps_to_no_bytes() {
        let (path, file) = file_with(&[], "empty");
        let map = map_read_only(&file).expect("map");
        assert!(map.is_empty());
        std::fs::remove_file(path).expect("remove");
    }

    #[test]
    fn a_file_that_cannot_be_read_is_refused() {
        let (path, _) = file_with(b"x", "write_only");
        let write_only = std::fs::OpenOptions::new().write(true).open(&path).expect("open");
        assert!(map_read_only(&write_only).is_err(), "a read map of a write-only descriptor");
        std::fs::remove_file(path).expect("remove");
    }
}
