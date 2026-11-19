//! The capture and supervision facts of each cycle input binding row.

use hypha_core::introspection::Supervision;
use hypha_cycle::CycleRuntime;

use super::types::HyphaCycleBindingOptions;
use crate::introspection::{IntrospectionRow, input_mode};

/// Copy the capture options and the current supervision of input `index` into its row.
pub(super) fn refresh_input_row(
    rows: &[IntrospectionRow],
    options: &[HyphaCycleBindingOptions],
    runtime: &CycleRuntime,
    index: usize,
) {
    let Some((row, selected)) = rows.get(index).zip(options.get(index)) else {
        return;
    };
    row.set_input_capture(input_mode(selected));
    let supervision = runtime.binding_capture(index).and_then(|capture| capture.supervision());
    let facts = supervision.map(|supervision| {
        let limits = supervision.limits();
        Supervision {
            silence_nanos: limits.silence_nanos,
            stagnation_nanos: limits.stagnation_nanos,
            ignored: supervision.ignored(),
        }
    });
    row.set_input_supervision(facts);
}
