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
