//! Supervision of one input binding: the silence and stagnation limits, their
//! declaration rules and their attach into the cycle. The silence limit is the
//! binding's only age limit.

use std::os::raw::c_char;

use super::registration::CycleStep;
use super::state::CycleId;
use super::types::HyphaSupervisionLimits;
use crate::node::HyphaNode;
use crate::port_limits::{Declared, HyphaSupervisionRanges, supervise};
use crate::status::Error;

/// What one input's supervision binding reads: the port table, the checks the code
/// It is read for the call only.
///
/// `silence_ms` is required on an enabled input, advisory or not, unless `unsupervised`
/// is set, and refused with it; `silence_ms = 0` is an input with no time limit, as the
/// tag is. `stagnation_ms` is read when `check_stagnation` is set. A key the request does not
/// allow and a limit out of range are `InvalidArgument`; a declared limit that is
/// missing is `NotAvailable`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaInputSupervisionRequest {
    /// The node whose parameter file holds `ports.inputs.<name>`. Not null.
    pub node: *const HyphaNode,
    /// The port name, `name_len` UTF-8 bytes. Empty for an unnamed port, which
    /// accepts no declared check.
    pub name: *const c_char,
    /// The byte length of `name`.
    pub name_len: usize,
    /// Nonzero when the port is tagged `unsupervised`: it runs without `silence_ms`.
    pub unsupervised: u8,
    /// Nonzero when the port reads `stagnation_ms` (it has an alive counter).
    pub check_stagnation: u8,
    /// Nonzero when the port reads its topic from the table, so a `topic` key is allowed.
    pub takes_topic: u8,
    /// The bounds a limit must meet. A limit outside them is refused, never clamped.
    pub ranges: HyphaSupervisionRanges,
}

/// What the binding attached.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaInputSupervisionResult {
    /// Nonzero when limits attached: the input is supervised.
    pub attached: u8,
    /// The table's `ignored` flag. Zero unless `attached` is nonzero.
    pub ignored: u8,
    /// The limits that attached, in nanoseconds; zero when `attached` is zero.
    pub limits: HyphaSupervisionLimits,
}

/// Reads and attaches the supervision of `port_name` on binding `binding_index` of
/// `cycle`: the attached limits, `None` when nothing is attached, or a refusal with its reason.
///
/// # Errors
/// The refusal and the text that names its cause.
pub fn cycle_bind_input_supervision(
    set: &crate::waitset::HyphaWaitSet,
    cycle: CycleId,
    session: &HyphaNode,
    request: &HyphaInputSupervisionRequest,
    port_name: &str,
    binding_index: u32,
) -> Result<Option<HyphaInputSupervisionResult>, (Error, String)> {
    let cycle = set.cycle(cycle).map_err(|status| (status, String::new()))?;
    let mut registration = cycle.configurable(set).map_err(|status| (status, String::new()))?;
    let registration: &mut CycleStep = &mut registration;
    let input = registration
        .inputs
        .get(binding_index as usize)
        .ok_or((Error::InvalidArgument, String::new()))?;
    let enabled = input.history.is_some();
    let declared = Declared {
        unsupervised: request.unsupervised != 0,
        stagnation: request.check_stagnation != 0,
    };
    if port_name.is_empty() {
        return unnamed_port(declared, enabled);
    }
    if let Some(name) = registration.port_names.get_mut(binding_index as usize) {
        port_name.clone_into(name);
    }
    let entry = format!("ports.inputs.{port_name}");
    let supervision =
        supervise(session, &entry, declared, request.takes_topic != 0, enabled, &request.ranges)
            .map_err(|(status, text)| (Error::from_status(status), text))?;
    let has_limit = supervision.limits.silence_nanos != 0 || declared.stagnation;
    let attach_limits = has_limit && enabled;
    if !attach_limits {
        return Ok(None);
    }
    attach_supervision(registration, binding_index, supervision.limits, supervision.ignored)
        .map_err(|error| (error, String::new()))?;
    Ok(Some(HyphaInputSupervisionResult {
        attached: 1,
        ignored: u8::from(supervision.ignored),
        limits: supervision.limits,
    }))
}

/// The rule for a port without a table: it reads no key, so an enabled input, advisory
/// or not, has no time limit and must say so with the tag `unsupervised`.
fn unnamed_port(
    declared: Declared,
    enabled: bool,
) -> Result<Option<HyphaInputSupervisionResult>, (Error, String)> {
    if declared.stagnation {
        return Err((Error::InvalidArgument, "supervision needs a named port".to_owned()));
    }
    let needs_tag = enabled && !declared.unsupervised;
    if needs_tag {
        return Err((
            Error::InvalidArgument,
            "an input without a table has no time limit and needs the tag hypha::unsupervised"
                .to_owned(),
        ));
    }
    Ok(None)
}

/// Attach limits to input `binding_index`. A disabled input attaches nothing.
fn attach_supervision(
    registration: &mut CycleStep,
    binding_index: u32,
    limits: HyphaSupervisionLimits,
    ignored: bool,
) -> Result<(), Error> {
    let Some(input) = registration.inputs.get(binding_index as usize) else {
        return Err(Error::InvalidArgument);
    };
    let (Ok(silence_nanos), Ok(stagnation_nanos)) =
        (u64::try_from(limits.silence_nanos), u64::try_from(limits.stagnation_nanos))
    else {
        return Err(Error::InvalidArgument);
    };
    let no_check = silence_nanos == 0 && stagnation_nanos == 0;
    if no_check {
        return Err(Error::InvalidArgument);
    }
    let disabled_input = input.history.is_none();
    if disabled_input {
        return Ok(());
    }
    let supervision = hypha_cycle::SupervisionLimits { silence_nanos, stagnation_nanos };
    registration
        .runtime
        .set_supervision(binding_index as usize, supervision, ignored)
        .map_err(Error::from_code)?;
    super::introspection::refresh_input_row(
        &registration.input_rows,
        &registration.input_options,
        &registration.runtime,
        binding_index as usize,
    );
    Ok(())
}
