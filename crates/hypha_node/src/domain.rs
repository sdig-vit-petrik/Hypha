//! The domain this process belongs to.

use crate::status::Error;

/// The domain of this process, empty for the domain "none".
///
/// # Errors
/// [`Error::InvalidArgument`] when `HYPHA_DOMAIN` holds an invalid name.
pub fn domain() -> Result<&'static str, Error> {
    hypha_iceoryx2::domain::domain().map_err(Error::from)
}
