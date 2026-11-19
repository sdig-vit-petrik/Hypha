//! A read-only memory map of a file.

use std::fs::File;
use std::ops::Deref;

/// The bytes of a file, mapped read-only. Reading them reads the file.
#[derive(Debug)]
pub struct ReadOnlyMap(memmap2::Mmap);

impl Deref for ReadOnlyMap {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

/// Map all of `file` read-only, for data that can be larger than memory.
///
/// Sound only while no process changes the file for as long as the map lives: a write
/// changes borrowed bytes, and a truncation makes a read end the process with SIGBUS.
/// Its one user, the player of `hypha_tools`, maps only complete recordings.
///
/// # Errors
/// The error of `mmap`.
pub fn map_read_only(file: &File) -> std::io::Result<ReadOnlyMap> {
    // SAFETY: only a complete recording is mapped, and nothing writes it meanwhile.
    let map = unsafe { memmap2::MmapOptions::new().map(file) }?;
    Ok(ReadOnlyMap(map))
}
