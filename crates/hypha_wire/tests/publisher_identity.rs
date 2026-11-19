//! Publisher identifiers never run out: the ordinal wraps.

use hypha_wire::integrity::{next_publisher_id, publisher_id_from};

#[test]
fn a_publisher_identifier_is_always_handed_out() {
    let first: u64 = next_publisher_id(1_000, 42);
    let second: u64 = next_publisher_id(1_000, 42);
    assert_ne!(first, second);
}

#[test]
fn the_ordinals_on_both_sides_of_the_wrap_give_distinct_identifiers() {
    let seed = 0x1234_5678_9ABC_DEF1_u64;
    let ordinals = [u64::MAX - 1, u64::MAX, 0, 1];
    for (index, left) in ordinals.iter().enumerate() {
        for right in ordinals.iter().skip(index.saturating_add(1)) {
            assert_ne!(publisher_id_from(seed, *left), publisher_id_from(seed, *right));
        }
    }
}
