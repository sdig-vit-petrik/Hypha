//! The committed `McapReplay` fixtures are exactly what the example `golden_fixture` writes.

#![expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]

#[path = "../examples/golden_fixture/fixture.rs"]
mod fixture;

use std::path::Path;

fn committed(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../hypha-cxx/test/data").join(name);
    std::fs::read(&path).expect("the committed fixture is readable")
}

fn assert_same(name: &str, built: &[u8]) {
    let file = committed(name);
    assert!(
        file == built,
        "crates/hypha-cxx/test/data/{name} differs from what its writer builds; run \
         `cargo run -p hypha_testing_mcap --example golden_fixture` and commit the result"
    );
}

#[test]
fn the_committed_plain_fixture_is_what_the_writer_builds() {
    assert_same(fixture::PLAIN_FILE, &fixture::plain().expect("the plain fixture is built"));
}

#[test]
fn the_committed_zstd_fixture_is_what_the_writer_builds() {
    assert_same(fixture::ZSTD_FILE, &fixture::zstd().expect("the zstd fixture is built"));
}

#[test]
fn the_writer_builds_the_same_bytes_every_time() {
    assert_eq!(fixture::plain().expect("first"), fixture::plain().expect("second"));
    assert_eq!(fixture::zstd().expect("first"), fixture::zstd().expect("second"));
}
