//! `hypha_msggen`: generates C++ or Rust types from ROS `.msg` and `.srv` definitions
//! for one package, or with `--language ros2` the ROS 2 alias headers of the C++ ones.
//! Called by `hypha_generate_messages` and `hypha_generate_ros2_headers` in `CMake`,
//! not by hand.

use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use hypha_msgdef::parse::{parse_message, parse_service};
use hypha_msgdef::resolve::{SpecSet, definition_text, message_identity, service_identity};
use hypha_msgdef::spec::{ServiceSpec, TypeName};
use hypha_msggen::emit_cpp::emit_message_with_definition;
use hypha_msggen::emit_ros2::{emit_message_alias, emit_service_alias};
use hypha_msggen::naming::{camel_to_snake, ros2_header_stem};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Language {
    Cpp,
    Rust,
    /// `<pkg>/msg/<name>.hpp` and `<pkg>/srv/<name>.hpp` over the C++ headers.
    Ros2,
}

struct Dependency {
    package: String,
    dir: PathBuf,
}

struct Cli {
    package: String,
    msg_dir: Option<PathBuf>,
    srv_dir: Option<PathBuf>,
    /// The definitions the package already has (an SDK package the application adds to):
    /// read for resolution, never emitted, and not to be defined again.
    base_dir: Option<PathBuf>,
    dependencies: Vec<Dependency>,
    out_dir: PathBuf,
    language: Language,
}

const USAGE: &str = "usage: hypha_msggen --package <name> [--msg-dir <dir>] [--srv-dir <dir>] \
[--base-dir <dir>] [--dependency <package>=<dir>...] --out-dir <dir> [--language cpp|rust|ros2]";

/// A package name becomes a directory and a C++ namespace, so it holds no path.
fn is_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts_well = chars.next().is_some_and(|first| first.is_ascii_lowercase());
    starts_well
        && chars.all(|next| next.is_ascii_lowercase() || next.is_ascii_digit() || next == '_')
}

fn parse_dependency(value: &str) -> Result<Dependency, Box<dyn Error>> {
    let (package, dir) = value
        .split_once('=')
        .filter(|(package, dir)| !package.is_empty() && !dir.is_empty())
        .ok_or_else(|| format!("--dependency needs <package>=<dir>, got '{value}'"))?;
    Ok(Dependency { package: package.to_string(), dir: PathBuf::from(dir) })
}

fn parse_args() -> Result<Cli, Box<dyn Error>> {
    let mut package = None;
    let mut msg_dir = None;
    let mut srv_dir = None;
    let mut base_dir = None;
    let mut dependencies = Vec::new();
    let mut out_dir = None;
    let mut language = Language::Cpp;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--package" => {
                package = Some(args.next().ok_or("--package needs a value")?);
            }
            "--msg-dir" => {
                msg_dir = Some(PathBuf::from(args.next().ok_or("--msg-dir needs a value")?));
            }
            "--srv-dir" => {
                srv_dir = Some(PathBuf::from(args.next().ok_or("--srv-dir needs a value")?));
            }
            "--base-dir" => {
                base_dir = Some(PathBuf::from(args.next().ok_or("--base-dir needs a value")?));
            }
            "--dependency" => {
                let value = args.next().ok_or("--dependency needs a value")?;
                dependencies.push(parse_dependency(&value)?);
            }
            "--language" => {
                language = match args.next().as_deref() {
                    Some("cpp") => Language::Cpp,
                    Some("rust") => Language::Rust,
                    Some("ros2") => Language::Ros2,
                    Some(value) => return Err(format!("unsupported language '{value}'").into()),
                    None => return Err("--language needs cpp, rust or ros2".into()),
                };
            }
            "--out-dir" => {
                out_dir = Some(PathBuf::from(args.next().ok_or("--out-dir needs a value")?));
            }
            other => return Err(format!("unrecognised argument '{other}'. {USAGE}").into()),
        }
    }

    let package = package.ok_or(USAGE)?;
    let out_dir = out_dir.ok_or(USAGE)?;
    if msg_dir.is_none() && srv_dir.is_none() {
        return Err(USAGE.into());
    }
    let named_packages =
        std::iter::once(&package).chain(dependencies.iter().map(|dependency| &dependency.package));
    for name in named_packages {
        if !is_package_name(name) {
            return Err(format!(
                "'{name}' is not a package name: use lowercase letters, digits and underscores"
            )
            .into());
        }
    }
    if dependencies.iter().any(|dependency| dependency.package == package) {
        return Err(format!("'{package}' is both the generated package and a dependency").into());
    }
    for (index, dependency) in dependencies.iter().enumerate() {
        let named_before =
            dependencies.iter().take(index).any(|earlier| earlier.package == dependency.package);
        if named_before {
            return Err(
                format!("dependency '{}' is given more than once", dependency.package).into()
            );
        }
    }
    Ok(Cli { package, msg_dir, srv_dir, base_dir, dependencies, out_dir, language })
}

struct MessageSource {
    package: String,
    name: String,
    path: PathBuf,
}

/// The definitions of one package: every `extension` file directly in `dir`.
fn discover_in_dir(
    package: &str,
    dir: &Path,
    extension: &str,
) -> Result<Vec<MessageSource>, Box<dyn Error>> {
    let mut sources = Vec::new();
    let entries =
        fs::read_dir(dir).map_err(|error| format!("reading {}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        let is_definition =
            path.is_file() && path.extension().and_then(OsStr::to_str) == Some(extension);
        if !is_definition {
            continue;
        }
        let Some(name) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        sources.push(MessageSource { package: package.to_string(), name: name.to_string(), path });
    }
    Ok(sources)
}

fn read_definition(source: &MessageSource) -> Result<String, Box<dyn Error>> {
    let path = &source.path;
    Ok(fs::read_to_string(path).map_err(|error| format!("reading {}: {error}", path.display()))?)
}

/// Parses every source into `specs` for resolution. `main` emits the owned sources only.
fn parse_into_specs(specs: &mut SpecSet, sources: &[MessageSource]) -> Result<(), Box<dyn Error>> {
    for src in sources {
        let type_name = TypeName::new(&src.package, &src.name);
        let text = read_definition(src)?;
        let spec = parse_message(type_name, &text)
            .map_err(|error| format!("parsing {}: {error}", src.path.display()))?;
        specs.insert(spec);
    }
    Ok(())
}

/// Runs the generator; an error is printed as its message text, not in debug form.
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = parse_args()?;

    let sources = match &cli.msg_dir {
        Some(dir) => discover_in_dir(&cli.package, dir, "msg")?,
        None => Vec::new(),
    };
    let service_sources = match &cli.srv_dir {
        Some(dir) => discover_in_dir(&cli.package, dir, "srv")?,
        None => Vec::new(),
    };

    // The package's existing definitions: read for resolution only, and not defined again.
    let mut base_sources = Vec::new();
    if let Some(base_dir) = &cli.base_dir {
        base_sources = discover_in_dir(&cli.package, base_dir, "msg")?;
        let base_services = discover_in_dir(&cli.package, base_dir, "srv")?;
        refuse_redefinition(
            &cli,
            base_dir,
            (&sources, &service_sources),
            &base_sources,
            &base_services,
        )?;
    }

    // Read for resolution only; their own package emits their headers.
    let mut dependency_sources = Vec::new();
    for dependency in &cli.dependencies {
        dependency_sources.extend(discover_in_dir(&dependency.package, &dependency.dir, "msg")?);
    }

    if sources.is_empty() && service_sources.is_empty() {
        return Err("no .msg or .srv files found for the package".into());
    }

    let mut specs = SpecSet::new();
    parse_into_specs(&mut specs, &sources)?;
    parse_into_specs(&mut specs, &base_sources)?;
    parse_into_specs(&mut specs, &dependency_sources)?;

    let mut services: Vec<ServiceSpec> = Vec::new();
    for src in &service_sources {
        let type_name = TypeName::new(&src.package, &src.name);
        let text = read_definition(src)?;
        let service = parse_service(type_name, &text)
            .map_err(|error| format!("parsing {}: {error}", src.path.display()))?;
        specs.insert(service.request.clone());
        specs.insert(service.response.clone());
        services.push(service);
    }

    for src in &sources {
        let type_name = TypeName::new(&src.package, &src.name);
        let spec = specs.get(&type_name).ok_or_else(|| format!("{}: spec vanished", src.name))?;
        let md5 = message_identity(&specs, &type_name)?;
        let def = definition_text(&specs, &type_name)?;
        let generated = match cli.language {
            Language::Cpp => emit_message_with_definition(spec, &md5, &def),
            Language::Rust => hypha_msggen::emit_rust::emit_message(spec, &md5),
            Language::Ros2 => emit_message_alias(&specs, spec)?,
        };

        let target_dir = package_dir(&cli, &src.package, "msg");
        fs::create_dir_all(&target_dir)
            .map_err(|error| format!("creating {}: {error}", target_dir.display()))?;

        let stem = output_stem(cli.language, &src.name);
        let out_path = target_dir.join(format!("{stem}.{}", extension(cli.language)));
        fs::write(&out_path, generated)
            .map_err(|error| format!("writing {}: {error}", out_path.display()))?;
        println!("hypha_msggen: wrote {}", out_path.display());
    }

    generate_services(&cli, &specs, &services)?;

    Ok(())
}

/// The type names one definition takes in its package: a message its own, a service its own
/// and those of its two halves (each has a header `<package>/<name>.h`).
fn taken_names(messages: &[MessageSource], services: &[MessageSource]) -> Vec<String> {
    let messages = messages.iter().map(|source| source.name.clone());
    let services = services.iter().flat_map(|source| {
        let name = &source.name;
        [name.clone(), format!("{name}Request"), format!("{name}Response")]
    });
    messages.chain(services).collect()
}

/// Defining a type of the package again would put two headers at one path, and two
/// types of one name and MD5 in one process: refused, naming the type.
fn refuse_redefinition(
    cli: &Cli,
    base_dir: &Path,
    (messages, services): (&[MessageSource], &[MessageSource]),
    base_messages: &[MessageSource],
    base_services: &[MessageSource],
) -> Result<(), Box<dyn Error>> {
    let existing = taken_names(base_messages, base_services);
    for name in taken_names(messages, services) {
        if existing.contains(&name) {
            return Err(format!(
                "{}/{name} is already defined by '{}': a definition added to a package of \
the SDK needs a name of its own",
                cli.package,
                base_dir.display()
            )
            .into());
        }
    }
    Ok(())
}

fn generate_services(
    cli: &Cli,
    specs: &SpecSet,
    services: &[ServiceSpec],
) -> Result<(), Box<dyn Error>> {
    for service in services {
        let target_dir = package_dir(cli, &service.type_name.package, "srv");
        fs::create_dir_all(&target_dir)?;
        let name = &service.type_name.name;
        let halves = || [format!("{name}Request"), format!("{name}Response"), name.clone()];
        let files: Vec<(String, String)> = match cli.language {
            Language::Cpp => {
                let identity = service_identity(specs, service)?;
                let output = hypha_msggen::emit_cpp::emit_service(specs, service, &identity)?;
                halves()
                    .into_iter()
                    .zip([output.request, output.response, output.service])
                    .collect()
            }
            Language::Rust => {
                let identity = service_identity(specs, service)?;
                let output = hypha_msggen::emit_rust::emit_service(service, &identity);
                halves()
                    .into_iter()
                    .zip([output.request, output.response, output.service])
                    .collect()
            }
            Language::Ros2 => vec![(name.clone(), emit_service_alias(specs, service)?)],
        };
        for (file_name, generated) in files {
            let stem = output_stem(cli.language, &file_name);
            let out_path = target_dir.join(format!("{stem}.{}", extension(cli.language)));
            fs::write(&out_path, generated)?;
            println!("hypha_msggen: wrote {}", out_path.display());
        }
    }

    Ok(())
}

/// `<out>/<package>`, or for a ROS 2 alias `<out>/<package>/<kind>` (`msg` or `srv`).
fn package_dir(cli: &Cli, package: &str, kind: &str) -> PathBuf {
    let dir = cli.out_dir.join(package);
    if cli.language == Language::Ros2 { dir.join(kind) } else { dir }
}

fn extension(language: Language) -> &'static str {
    match language {
        Language::Cpp => "h",
        Language::Rust => "rs",
        Language::Ros2 => "hpp",
    }
}

fn output_stem(language: Language, name: &str) -> String {
    match language {
        Language::Cpp => name.to_string(),
        Language::Rust => camel_to_snake(name),
        Language::Ros2 => ros2_header_stem(name),
    }
}
