//! Writes the golden-sample fixture of the C++ `hypha::testing::McapReplay` tests, and
//! its zstd-compressed copy, to `crates/hypha-cxx/test/data/`:
//!
//! ```text
//! cargo run -p hypha_testing_mcap --example golden_fixture
//! ```
//!
//! The test `tests/golden_fixture.rs` builds both in memory and compares them byte for
//! byte with the committed files, so a change here needs this run and a commit of both.

mod fixture;

use std::path::Path;

fn main() -> Result<(), String> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../hypha-cxx/test/data");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let files = [(fixture::PLAIN_FILE, fixture::plain()?), (fixture::ZSTD_FILE, fixture::zstd()?)];
    for (name, bytes) in files {
        let path = directory.join(name);
        std::fs::write(&path, bytes)
            .map_err(|error| format!("cannot write '{}': {error}", path.display()))?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
