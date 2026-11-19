//! Every message in the repository keeps its golden MD5 (`fixtures/golden_md5.txt`).

use std::fs;
use std::path::{Path, PathBuf};

use hypha_msgdef::parse::parse_message;
use hypha_msgdef::resolve::{SpecSet, message_identity};
use hypha_msgdef::spec::TypeName;

#[allow(clippy::expect_used)]
fn collect_msg_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("message directory is readable");
    for entry in entries {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            collect_msg_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "msg") {
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
    collect_msg_files(&msg_root, &mut files);
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
