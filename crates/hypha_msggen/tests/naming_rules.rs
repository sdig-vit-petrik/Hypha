//! Naming rules shared by the language emitters.

use hypha_msggen::naming::camel_to_snake;

#[test]
fn preserves_acronyms_as_words() {
    assert_eq!(camel_to_snake("GPSFix"), "gps_fix");
    assert_eq!(camel_to_snake("AddTwoIntsRequest"), "add_two_ints_request");
}
