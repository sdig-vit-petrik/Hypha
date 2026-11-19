//! Shared test driver for [`CycleRuntime`].

use hypha_cycle::{BindingCapture, CycleRuntime, CycleTime, OutputError, StagedMessage};

/// Runs one iteration whose step returns its emissions as `(output, payload)` pairs.
pub(crate) trait RunIteration {
    fn run_iteration<ReadClock, S, D>(
        &mut self,
        time: CycleTime,
        callback_start: ReadClock,
        message_stamp: Option<i64>,
        step: S,
        send: D,
    ) -> Result<(), i32>
    where
        ReadClock: FnOnce() -> i64,
        S: FnOnce(&[BindingCapture], CycleTime) -> Result<Vec<(u32, Vec<u8>)>, i32>,
        D: FnMut(u32, &[u8]) -> Result<(), OutputError>;
}

impl RunIteration for CycleRuntime {
    fn run_iteration<ReadClock, S, D>(
        &mut self,
        time: CycleTime,
        callback_start: ReadClock,
        message_stamp: Option<i64>,
        step: S,
        mut send: D,
    ) -> Result<(), i32>
    where
        ReadClock: FnOnce() -> i64,
        S: FnOnce(&[BindingCapture], CycleTime) -> Result<Vec<(u32, Vec<u8>)>, i32>,
        D: FnMut(u32, &[u8]) -> Result<(), OutputError>,
    {
        let mut emissions = Vec::new();
        self.run_iteration_with_emissions(
            time,
            callback_start,
            message_stamp,
            &mut emissions,
            |captures, time, output, budget| {
                for (index, bytes) in step(captures, time)? {
                    budget.admit(index, bytes.len())?;
                    output.push(StagedMessage { output: index, len: bytes.len(), value: bytes });
                }
                Ok(())
            },
            |index, bytes: Vec<u8>| send(index, &bytes),
        )
    }
}
