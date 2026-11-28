//! Build script for `hypha_msgs`: generates Rust modules for every `.msg` in `msg/`.

use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use hypha_msgdef::parse::parse_message;
use hypha_msgdef::resolve::{SpecSet, message_identity};
use hypha_msgdef::spec::TypeName;
use hypha_msggen::naming::camel_to_snake;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=msg");

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let msg_root = manifest_dir.join("msg");

    let mut specs = SpecSet::new();
    let mut packages = Vec::new();

    let entries = fs::read_dir(&msg_root)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let pkg_name = entry.file_name().to_string_lossy().to_string();
            packages.push(pkg_name.clone());

            let msg_files = fs::read_dir(&path)?;
            for msg_entry in msg_files {
                let msg_entry = msg_entry?;
                let msg_path = msg_entry.path();
                if msg_path.extension().is_some_and(|ext| ext == "msg") {
                    let Some(stem) = msg_path.file_stem() else {
                        continue;
                    };
                    let name = stem.to_string_lossy().to_string();
                    let type_name = TypeName::new(&pkg_name, &name);
                    let content = fs::read_to_string(&msg_path)?;
                    let spec = parse_message(type_name, &content)
                        .map_err(|e| format!("parsing {}: {e:?}", msg_path.display()))?;
                    specs.insert(spec);
                }
            }
        }
    }

    packages.sort();

    let mut lib_generated = String::new();

    for pkg in &packages {
        let pkg_out_dir = out_dir.join(pkg);
        fs::create_dir_all(&pkg_out_dir)?;

        let _ = writeln!(lib_generated, "/// Standard `{pkg}` messages.\npub mod {pkg} {{");

        let mut pkg_specs: Vec<_> =
            specs.values().filter(|s| s.type_name.package == *pkg).collect();
        pkg_specs.sort_by(|a, b| a.type_name.name.cmp(&b.type_name.name));

        for spec in pkg_specs {
            let md5 = message_identity(&specs, &spec.type_name)?;
            let rust_source = hypha_msggen::emit_rust::emit_message(spec, &md5);

            let snake_name = camel_to_snake(&spec.type_name.name);
            let out_file = pkg_out_dir.join(format!("{snake_name}.rs"));
            fs::write(&out_file, rust_source)?;

            let _ = writeln!(lib_generated, "    #[path = \"{}\"]", out_file.display());
            let _ = writeln!(lib_generated, "    pub mod {snake_name};");
            let _ = writeln!(lib_generated, "    pub use {snake_name}::{};", spec.type_name.name);
        }

        lib_generated.push_str("}\n\n");
    }

    let lib_gen_path = out_dir.join("lib_generated.rs");
    fs::write(&lib_gen_path, lib_generated)?;
    Ok(())
}
