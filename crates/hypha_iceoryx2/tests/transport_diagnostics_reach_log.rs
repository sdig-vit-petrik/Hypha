//! A transport diagnostic reaches the `hypha_log` file sinks as `hypha/transport`.
#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hypha_core::port::PortError;
use hypha_iceoryx2::domain::{DOMAIN_VARIABLE, config};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_invalid_domain_is_written_to_the_log_file_with_the_transport_module() -> TestResult {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir: PathBuf =
        std::env::temp_dir().join(format!("hypha_transport_log_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let table: toml::Table = "console = false\n[sink.main]\npath = \"transport.log\"\n".parse()?;
    let prepared =
        hypha_log::prepare(&table, &dir.join("node.toml")).map_err(|error| format!("{error:?}"))?;
    hypha_log::commit(prepared).expect("commit");

    hypha_testkit::set_env(DOMAIN_VARIABLE, "Not A Domain");
    assert!(matches!(config(), Err(PortError::InvalidDomain)));

    let text = std::fs::read_to_string(dir.join("transport.log"))?;
    assert!(text.contains("[hypha/transport]"), "log file holds: {text}");
    assert!(text.contains("is not a domain name"), "log file holds: {text}");
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
