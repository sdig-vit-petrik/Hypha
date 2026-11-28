//! A parameter file whose `[log]` table or `node.name` is refused fails to load, before
//! any node exists.

use hypha_node::safe::config_create;
use hypha_node::status::Error;
use hypha_testkit::unique_name;

fn load(parameters: &str) -> Result<(), Error> {
    let path = std::env::temp_dir().join(format!("{}.toml", unique_name("config_values")));
    std::fs::write(&path, parameters).map_err(|_| Error::Backend)?;
    let loaded = config_create(path.to_str());
    let _removed = std::fs::remove_file(path);
    loaded.map(drop)
}

#[test]
fn a_log_table_that_is_refused_fails_the_load() {
    assert_eq!(load("[log]\nlevel = \"loud\"\n"), Err(Error::InvalidArgument));
    assert_eq!(load("[log]\nlevel = \"warn\"\n"), Ok(()));
}

#[test]
fn a_node_name_that_is_refused_fails_the_load() {
    assert_eq!(load("[node]\nname = \"Bad Name\"\n"), Err(Error::InvalidArgument));
    assert_eq!(load("[node]\nname = 5\n"), Err(Error::InvalidArgument));
    assert_eq!(load("[node]\nname = \"good_name\"\n"), Ok(()));
}

#[test]
fn a_slow_callback_budget_that_is_refused_fails_the_load() {
    assert_eq!(load("[log]\nslow_callback_ms = \"fast\"\n"), Err(Error::InvalidArgument));
    assert_eq!(load("[log]\nslow_callback_ms = -5\n"), Err(Error::InvalidArgument));
    assert_eq!(load("[log]\nslow_callback_ms = 50\n"), Ok(()));
    assert_eq!(load("[log]\nslow_callback_ms = 0\n"), Ok(()));
}
