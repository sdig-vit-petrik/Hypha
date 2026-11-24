//! Naming rules shared by the language emitters.

use hypha_msggen::naming::camel_to_snake;

#[test]
fn preserves_acronyms_as_words() {
    assert_eq!(camel_to_snake("GPSFix"), "gps_fix");
    assert_eq!(camel_to_snake("AddTwoIntsRequest"), "add_two_ints_request");
}

#[test]
fn ros2_header_stem_follows_rosidl() {
    use hypha_msggen::naming::ros2_header_stem;
    let cases = [
        ("Header", "header"),
        ("PointCloud2", "point_cloud2"),
        ("Point32", "point32"),
        ("Vector3Stamped", "vector3_stamped"),
        ("TFMessage", "tf_message"),
        ("NavSatFix", "nav_sat_fix"),
        ("GPSFix", "gps_fix"),
        ("NavigationSolutionGsof49", "navigation_solution_gsof49"),
        ("UUID", "uuid"),
    ];
    for (name, stem) in cases {
        assert_eq!(ros2_header_stem(name), stem, "{name}");
    }
}
