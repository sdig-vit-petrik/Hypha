//! `slow_callback_ms` of the `[log]` table: validated here, read by the node, not part of
//! the process's logging configuration (#128).
#![allow(clippy::expect_used)]

mod support;

use hypha_log::{LogError, slow_callback_ms, validate};
use support::{TestResult, log_table, parameter_file, temp_dir};

#[test]
fn the_budget_is_read_as_given_and_absent_without_the_key() -> TestResult {
    let dir = temp_dir("slow_callback_read")?;
    let file = parameter_file(&dir);
    for (source, expected) in [
        ("[log]\nlevel = \"info\"\n", None),
        ("[log]\nslow_callback_ms = 50\n", Some(50)),
        ("[log]\nslow_callback_ms = 0\n", Some(0)),
        ("[log]\nslow_callback_ms = 3600000\n", Some(3_600_000)),
    ] {
        let table = log_table(source)?;
        validate(&table, &file).map_err(|error| format!("{source}: {error:?}"))?;
        assert_eq!(slow_callback_ms(&table), expected, "{source}");
    }
    Ok(())
}

#[test]
fn a_budget_that_is_no_whole_number_of_milliseconds_is_refused_naming_the_key() -> TestResult {
    let dir = temp_dir("slow_callback_refused")?;
    let file = parameter_file(&dir);
    for source in [
        "[log]\nslow_callback_ms = -1\n",
        "[log]\nslow_callback_ms = 3600001\n",
        "[log]\nslow_callback_ms = \"50\"\n",
        "[log]\nslow_callback_ms = 0.5\n",
    ] {
        let refused = validate(&log_table(source)?, &file);
        let Err(LogError::InvalidArgument(message)) = refused else {
            return Err(format!("{source}: expected InvalidArgument, got {refused:?}").into());
        };
        assert!(message.contains("slow_callback_ms"), "{message}");
    }
    Ok(())
}
