//! Link metadata for the C ABI shared library: a soname without a directory, so an
//! installed copy is found through the loader's search path.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let revision_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../abi.revision");
    println!("cargo:rerun-if-changed={}", revision_path.display());
    let revision_text = std::fs::read_to_string(&revision_path)?;
    let revision = revision_text.trim().parse::<u32>()?;
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(
        out_dir.join("abi_revision.rs"),
        format!("const ABI_REVISION: u32 = {revision};\n"),
    )?;
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libhypha_ffi.so.{revision}");
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libhypha_ffi.{revision}.dylib");
    Ok(())
}
