//! A process whose `/dev/shm` marker differs from the domain's is refused.
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used)]

use hypha_core::port::PortError;
use hypha_iceoryx2::Iox2Transport;
use hypha_iceoryx2::domain::DOMAIN_VARIABLE;

#[test]
fn a_different_shm_marker_refuses_the_node() {
    let domain = format!("t_{}_mount", std::process::id());
    let marker = format!("iox2-{domain}-hypha_mount_id");
    let shm = std::path::Path::new("/dev/shm").join(&marker);
    let meta = std::path::Path::new("/tmp/iceoryx2").join(&marker);
    std::fs::write(&shm, "written_by_another_mount").expect("shm marker");
    hypha_testkit::set_env(DOMAIN_VARIABLE, &domain);

    let refused = Iox2Transport::new();

    let _ = std::fs::remove_file(&shm);
    let _ = std::fs::remove_file(&meta);
    assert!(matches!(refused, Err(PortError::InvalidState)), "{:?}", refused.err());
}
