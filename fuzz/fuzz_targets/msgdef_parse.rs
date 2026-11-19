//! The message definition parser and resolution on arbitrary text.
//! Input: the definition text (invalid UTF-8 is replaced, so every input is text).
#![no_main]

use hypha_msgdef::definition::parse_definition;
use hypha_msgdef::md5::md5_from_definition;
use hypha_msgdef::parse::{parse_message, parse_service};
use hypha_msgdef::resolve::{check_expansion, definition_text, message_identity};
use hypha_msgdef::size::verified_encoded_size;
use hypha_msgdef::spec::TypeName;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let text = String::from_utf8_lossy(input);
    let root = TypeName::new("p", "T0");
    let _ = parse_message(root.clone(), &text);
    let _ = parse_service(root.clone(), &text);
    if let Ok(specs) = parse_definition(&root, &text) {
        let _ = check_expansion(&specs, &root);
        let canonical = definition_text(&specs, &root);
        let identity = message_identity(&specs, &root);
        if let (Ok(canonical), Ok(identity)) = (canonical, identity) {
            assert_eq!(md5_from_definition("p/T0", &canonical).as_ref(), Some(&identity));
            let reparsed = parse_definition(&root, &canonical).unwrap_or_else(|error| {
                panic!("the canonical text is refused: {error}\n{canonical}")
            });
            let rendered_again = definition_text(&reparsed, &root).ok();
            assert_eq!(rendered_again.as_deref(), Some(canonical.as_str()), "not a fixed point");
            assert_eq!(message_identity(&reparsed, &root).ok(), Some(identity));
        }
    }
    let _ = md5_from_definition("p/T0", &text);
    let _ = verified_encoded_size("p/T0", "00000000000000000000000000000000", &text);
});
