//! Shared source-language naming rules.

/// Convert a ROS `CamelCase` type name into a Rust module name.
#[must_use]
pub fn camel_to_snake(name: &str) -> String {
    let letters: Vec<char> = name.chars().collect();
    let mut result = String::new();

    for (index, letter) in letters.iter().enumerate() {
        let previous = index.checked_sub(1).and_then(|before| letters.get(before)).copied();
        let next = letters.get(index.saturating_add(1)).copied();
        let follows_lower_case = previous.is_some_and(char::is_lowercase);
        let ends_a_run_of_capitals =
            previous.is_some_and(char::is_uppercase) && next.is_some_and(char::is_lowercase);

        if letter.is_uppercase() && (follows_lower_case || ends_a_run_of_capitals) {
            result.push('_');
        }
        result.extend(letter.to_lowercase());
    }

    result
}

/// Convert a ROS `CamelCase` type name into the stem of its ROS 2 header, by
/// rosidl's rule: `Vector3Stamped` is `vector3_stamped`, `TFMessage` is `tf_message`.
///
/// It differs from [`camel_to_snake`] after a digit, which here ends a word.
#[must_use]
pub fn ros2_header_stem(name: &str) -> String {
    let letters: Vec<char> = name.chars().collect();
    let mut result = String::new();

    for (index, letter) in letters.iter().enumerate() {
        let previous = index.checked_sub(1).and_then(|before| letters.get(before)).copied();
        let next = letters.get(index.saturating_add(1)).copied();
        let follows_lower_case_or_digit =
            previous.is_some_and(|before| before.is_lowercase() || before.is_ascii_digit());
        let starts_a_word = previous.is_some() && next.is_some_and(char::is_lowercase);

        if letter.is_uppercase() && (follows_lower_case_or_digit || starts_a_word) {
            result.push('_');
        }
        result.extend(letter.to_lowercase());
    }

    result
}
