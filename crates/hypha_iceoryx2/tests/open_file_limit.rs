//! A low open-file limit is raised when the process first uses the domain.
#![allow(clippy::expect_used)]

use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

#[test]
fn the_first_use_of_the_domain_raises_a_low_open_file_limit() {
    let limit = getrlimit(Resource::Nofile);
    setrlimit(Resource::Nofile, Rlimit { current: Some(256), ..limit }).expect("lower the limit");
    hypha_iceoryx2::domain::config().expect("the domain");
    let soft = getrlimit(Resource::Nofile).current;
    let wanted = limit.maximum.map_or(10_240, |max| max.min(10_240));
    assert_eq!(soft, Some(wanted.max(256)));
}
