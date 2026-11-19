//! Whether a bounded publisher's pool fits into `/dev/shm` (Linux).
//!
//! The pool is a sparse tmpfs file, and a write past the free space of `/dev/shm`
//! raises SIGBUS with no error to return, so hypha warns at creation when a full
//! pool cannot fit.
//! In a bounded deployment (`domain::bounded_memory`) it refuses the publisher instead.

use hypha_core::port::PortError;

/// Bytes of one pool sample carrying `payload` bytes, headers and alignment included.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn sample_bytes(payload: usize) -> usize {
    use core::mem::{align_of, size_of};
    use iceoryx2::service::header::publish_subscribe::Header;

    let align = |value: usize, to: usize| value.div_ceil(to).saturating_mul(to);
    let frame = size_of::<crate::header::Iox2FrameHeader>();
    let headers = align(size_of::<Header>(), align_of::<crate::header::Iox2FrameHeader>())
        .saturating_add(frame);
    let alignment = align_of::<Header>().max(align_of::<crate::header::Iox2FrameHeader>());
    align(headers.saturating_add(payload), alignment)
}

/// The backend code of a check that could not read the free space of `/dev/shm`.
#[cfg(target_os = "linux")]
const CHECK_FAILED: u32 = 8;

/// Warn when a full pool of `samples` samples of `payload` bytes for `topic` cannot
/// become resident in the free space of `/dev/shm`. With `refuse_shortfall`, the
/// rule of a bounded deployment, a pool that cannot fit is an error instead.
///
/// # Errors
/// [`PortError::Backend`] when the free space cannot be read, and
/// [`PortError::ResourceLimit`] when `refuse_shortfall` is set and the pool cannot fit.
#[cfg(target_os = "linux")]
pub(crate) fn check(
    topic: &str,
    samples: usize,
    payload: usize,
    refuse_shortfall: bool,
) -> Result<(), PortError> {
    const SHM_DIR: &str = "/dev/shm";
    let stat = rustix::fs::statvfs(SHM_DIR).map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: topic '{topic}': cannot read the free space of {SHM_DIR}: {cause:?}");
        PortError::Backend(CHECK_FAILED)
    })?;
    let free = stat.f_bavail.saturating_mul(stat.f_frsize);
    let need = u64::try_from(samples.saturating_mul(sample_bytes(payload))).unwrap_or(u64::MAX);
    let pool_cannot_fit = need > free;
    if pool_cannot_fit && refuse_shortfall {
        log::error!(target: "hypha/transport",
            "hypha_iceoryx2: topic '{topic}' needs {need} bytes of shared memory for its publisher pool \
             ({samples} samples of {payload} bytes); /dev/shm has {free} bytes free, and a bounded deployment \
             refuses a publisher that cannot fit. Enlarge /dev/shm (container: shm_size or the tmpfs volume) or \
             lower the topic's payload limit.");
        return Err(PortError::ResourceLimit);
    }
    if pool_cannot_fit {
        log::warn!(target: "hypha/transport",
            "hypha_iceoryx2: topic '{topic}' may need {need} bytes of shared memory when its subscribers fall behind \
             ({samples} samples of {payload} bytes); /dev/shm has {free} bytes free, and a write past them stops the \
             process with SIGBUS. Enlarge /dev/shm (container: shm_size or the tmpfs volume) or lower the topic's \
             payload limit.");
    }
    Ok(())
}

/// Other systems hide the pool's file, so there is nothing to check.
#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn check(
    _topic: &str,
    _samples: usize,
    _payload: usize,
    _refuse_shortfall: bool,
) -> Result<(), PortError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sample_carries_104_bytes_of_headers_aligned_to_8() {
        assert_eq!(super::sample_bytes(8), 112);
        assert_eq!(super::sample_bytes(1024), 1128);
        assert_eq!(super::sample_bytes(1025), 1136);
        assert_eq!(super::sample_bytes(64 * 1024), 65_640);
    }
}
