//! The configuration loader on arbitrary TOML text. Each input is written to one
//! `root-<pid>.toml` in the system temp directory and removed after the load.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use hypha_core::parameters::Parameters;
use libfuzzer_sys::fuzz_target;

fn root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::temp_dir().join(format!("hypha_fuzz_config_root-{}.toml", std::process::id()))
    })
}

fuzz_target!(|input: &[u8]| {
    let path = root();
    std::fs::write(path, input).expect("the fuzz input file");
    let loaded = Parameters::from_toml_file(path);
    std::fs::remove_file(path).expect("the fuzz input file is removed");
    let Ok(params) = loaded else {
        return;
    };
    for name in params.names() {
        assert!(params.value(name).is_some(), "{name} is named but has no value");
    }
    for name in params.list_names() {
        let len = params.list_len(name).expect("a listed name has a length");
        for index in 0..len {
            assert!(params.list_element(name, index).is_some(), "{name}[{index}] is missing");
        }
        assert!(params.list_element(name, len).is_none(), "{name} has more than its length");
        assert!(!params.has(name), "{name} is a list and a scalar");
    }
    let _ = params.unread_keys();
});
