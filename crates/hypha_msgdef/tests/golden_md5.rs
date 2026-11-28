//! Every message in the repository keeps its golden MD5 (`fixtures/golden_md5.txt`), and
//! every service its own (`fixtures/golden_srv_md5.txt`).

use std::fs;
use std::path::{Path, PathBuf};

use hypha_msgdef::parse::{parse_message, parse_service};
use hypha_msgdef::resolve::{SpecSet, message_identity, service_identity};
use hypha_msgdef::spec::TypeName;

#[allow(clippy::expect_used)]
fn collect_files(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("message directory is readable");
    for entry in entries {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            collect_files(&path, extension, out);
        } else if path.extension().is_some_and(|ext| ext == extension) {
            out.push(path);
        }
    }
}

#[test]
#[allow(clippy::expect_used)]
fn every_repository_message_keeps_its_md5() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let msg_root = manifest.join("../hypha_msgs/msg");
    let mut files = Vec::new();
    collect_files(&msg_root, "msg", &mut files);
    files.sort();

    let mut specs = SpecSet::new();
    let mut names = Vec::new();
    for file in &files {
        let package = file
            .parent()
            .and_then(Path::file_name)
            .expect("message file sits in a package directory")
            .to_string_lossy()
            .to_string();
        let name = file.file_stem().expect("message file has a name").to_string_lossy().to_string();
        let type_name = TypeName::new(&package, &name);
        let text = fs::read_to_string(file).expect("message file is readable");
        specs.insert(parse_message(type_name.clone(), &text).expect("message parses"));
        names.push(type_name);
    }
    assert!(names.len() > 50, "expected the repository messages, found {}", names.len());

    let mut actual: Vec<String> = names
        .iter()
        .map(|type_name| match message_identity(&specs, type_name) {
            Ok(md5) => format!("{} {md5}", type_name.full()),
            Err(error) => format!("{} ERROR {error:?}", type_name.full()),
        })
        .collect();
    actual.sort();

    let fixture = manifest.join("tests/fixtures/golden_md5.txt");
    let expected_text = fs::read_to_string(fixture).expect("golden fixture is readable");
    let expected: Vec<String> = expected_text.lines().map(String::from).collect();
    assert_eq!(actual, expected);
}

/// The services are the standard ones of ROS 1: each golden value is the MD5 that ROS 1's
/// `genmsg` computes for the same `.srv` (the ones of `crates/hypha_msgs/upstream` unchanged),
/// not only the one this generator printed. A change of a `.srv` or of the rules moves it.
#[test]
#[allow(clippy::expect_used)]
fn every_repository_service_keeps_its_md5() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let msg_root = manifest.join("../hypha_msgs/msg");
    let mut message_files = Vec::new();
    collect_files(&msg_root, "msg", &mut message_files);
    let mut service_files = Vec::new();
    collect_files(&msg_root, "srv", &mut service_files);
    service_files.sort();

    let package_of = |file: &Path| {
        file.parent()
            .and_then(Path::file_name)
            .expect("definition file sits in a package directory")
            .to_string_lossy()
            .to_string()
    };
    let name_of =
        |file: &Path| file.file_stem().expect("file has a name").to_string_lossy().to_string();

    let mut specs = SpecSet::new();
    for file in &message_files {
        let type_name = TypeName::new(&package_of(file), &name_of(file));
        let text = fs::read_to_string(file).expect("message file is readable");
        specs.insert(parse_message(type_name, &text).expect("message parses"));
    }
    let mut services = Vec::new();
    for file in &service_files {
        let type_name = TypeName::new(&package_of(file), &name_of(file));
        let text = fs::read_to_string(file).expect("service file is readable");
        let service = parse_service(type_name, &text).expect("service parses");
        specs.insert(service.request.clone());
        specs.insert(service.response.clone());
        services.push(service);
    }
    assert!(services.len() >= 11, "expected the repository services, found {}", services.len());

    let mut actual: Vec<String> = services
        .iter()
        .map(|service| match service_identity(&specs, service) {
            Ok(identity) => format!("{} {}", service.type_name.full(), identity.service),
            Err(error) => format!("{} ERROR {error:?}", service.type_name.full()),
        })
        .collect();
    actual.sort();

    let fixture = manifest.join("tests/fixtures/golden_srv_md5.txt");
    let expected_text = fs::read_to_string(fixture).expect("golden fixture is readable");
    let expected: Vec<String> = expected_text.lines().map(String::from).collect();
    assert_eq!(actual, expected);
}
