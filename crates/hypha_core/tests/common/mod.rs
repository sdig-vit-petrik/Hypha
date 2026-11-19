//! Load parameters from TOML text through the production file loader.

use std::sync::atomic::{AtomicUsize, Ordering};

use hypha_core::parameters::{ParameterLoadError, Parameters};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Write `source` to a fresh file called `name` and load it. The file is
/// removed again, so the returned error names a path that no longer exists.
#[allow(clippy::expect_used)]
pub(crate) fn from_text(source: &str, name: &str) -> Result<Parameters, ParameterLoadError> {
    let dir = std::env::temp_dir().join(format!(
        "hypha_core_text_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write fixture file");
    let loaded = Parameters::from_toml_file(&path);
    // Cleanup must not fail the test; a leftover temp directory is harmless.
    let _ = std::fs::remove_dir_all(&dir);
    loaded
}
