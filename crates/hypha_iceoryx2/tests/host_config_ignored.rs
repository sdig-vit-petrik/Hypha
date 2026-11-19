//! A host `iceoryx2.toml` must not change the domain's iceoryx2 configuration.
//! Its own test binary: the global configuration is set once per process.

#![allow(clippy::expect_used)]

use std::time::{SystemTime, UNIX_EPOCH};

use iceoryx2::config::Config;
use iceoryx2::prelude::{FilePath, SemanticString as _};

#[test]
fn a_host_config_file_does_not_change_the_domain_configuration() {
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    let moved_root = std::env::temp_dir().join(format!("hypha_host_root_{nanos}"));
    let config_dir = std::env::temp_dir().join(format!("hypha_host_config_{nanos}"));
    let config_path = config_dir.join("iceoryx2.toml");
    std::fs::create_dir_all(&config_dir).expect("create the temporary config directory");
    std::fs::write(&config_path, format!("[global]\nroot-path = \"{}/\"\n", moved_root.display()))
        .expect("write the temporary iceoryx2 config file");
    let file_path = FilePath::new(config_path.to_str().expect("UTF-8 path").as_bytes())
        .expect("a valid FilePath");
    Config::setup_global_config_from_file(&file_path)
        .expect("install the host file as the process-global iceoryx2 configuration");
    let _ = std::fs::remove_dir_all(&config_dir);

    let global_root =
        String::from_utf8_lossy(Config::global_config().global.root_path().as_bytes()).into_owned();
    assert!(
        global_root.starts_with(moved_root.to_str().expect("UTF-8 path")),
        "the global configuration was not installed: {global_root}"
    );

    let domain = hypha_iceoryx2::domain::config().expect("the domain configuration");
    let domain_root = String::from_utf8_lossy(domain.global.root_path().as_bytes()).into_owned();
    let default_root =
        String::from_utf8_lossy(Config::default().global.root_path().as_bytes()).into_owned();
    assert_eq!(domain_root, default_root, "the host file changed the domain's root path");
}
