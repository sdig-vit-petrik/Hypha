//! The C ABI wrapper of `hypha_node::node_checks`.

use crate::boundary::{borrow_str, handle_ref};
use crate::status::try_guard;
use hypha_node::safe::{HyphaNode, HyphaNodeCheck, check_row, node_set_checks};
use hypha_node::status::HyphaStatus;

/// Report the checks of `owner` to `node`: named facts of the application (the device is
/// connected, the fix is held), each with a level and a message, which `~/state` carries
/// in `hypha_msgs/NodeState.checks`. A check at `ERROR` or `STALE` makes the state of the
/// node `DEGRADED`, as a required input that is not ok does; `WARN` and `OK` show and
/// change nothing else.
///
/// The call replaces the whole set that `owner` reported before; `count` 0 removes it. An
/// owner is any number its caller picks and keeps (`hypha::DiagnosticUpdater` uses its
/// own address); owners do not see each other's checks, and a name may repeat. At most 64
/// checks in all, each name 1 to 128 bytes and each message at most 256 bytes, a longer one
/// cut and ended in `...`. `InvalidArgument`: a null `node`, a null `checks` with a count,
/// a null text with a length, text that is not UTF-8, an empty or too long name or a level
/// above 3. `TooLarge`: more than 64 checks in all. A refused call changes nothing and
/// records its reason for `hypha_last_error`. The texts are copied; the caller keeps them.
///
/// # Safety
/// `node` is live. `checks` is null or `count` readable `HyphaNodeCheck`, each with its
/// `name` and `message` pointing to as many readable bytes as their lengths say.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_node_set_checks(
    node: *const HyphaNode,
    owner: u64,
    checks: *const HyphaNodeCheck,
    count: usize,
) -> HyphaStatus {
    try_guard(|| {
        let session = unsafe { handle_ref(node) }?;
        let given: &[HyphaNodeCheck] = if count == 0 {
            &[]
        } else if checks.is_null()
            || count
                .checked_mul(size_of::<HyphaNodeCheck>())
                .is_none_or(|bytes| bytes > isize::MAX as usize)
        {
            return Err(HyphaStatus::InvalidArgument);
        } else {
            // SAFETY: non-null and a size no slice can exceed, by the checks; readable by
            // the caller.
            unsafe { std::slice::from_raw_parts(checks, count) }
        };
        let mut rows = Vec::with_capacity(given.len());
        for check in given {
            let name = unsafe { text(check.name, check.name_len) }?;
            let message = unsafe { text(check.message, check.message_len) }?;
            rows.push(check_row(name, check.level, message)?);
        }
        node_set_checks(session, owner, rows)?;
        Ok(())
    })
}

/// `len` bytes of text; null is the empty text for a length of zero.
///
/// # Safety
/// `bytes` is null or points to `len` readable bytes.
unsafe fn text<'a>(bytes: *const std::os::raw::c_char, len: usize) -> Result<&'a str, HyphaStatus> {
    if len == 0 {
        return Ok("");
    }
    unsafe { borrow_str(bytes, len) }
}
