//! `hypha_node::sdk::domain()` reports the domain `HYPHA_DOMAIN` selected.

#[test]
fn the_domain_is_the_one_the_environment_selected() {
    let expected = std::env::var("HYPHA_DOMAIN").unwrap_or_default();
    assert_eq!(hypha_node::sdk::domain(), Ok(expected));
}
