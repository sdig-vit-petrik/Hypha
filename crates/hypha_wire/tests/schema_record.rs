//! Schema record framing tests.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
use hypha_wire::schema_record::{
    MAX_SCHEMA_RECORD_LEN, SchemaRecordError, parse_schema_record, schema_record_frame_len,
    write_schema_record,
};

const TYPE: &str = "std_msgs/String";
const MD5: &str = "99222222222222222222222222222222";
const DEF: &str = "string data\n";

#[test]
fn schema_record_round_trips_with_footer() {
    let size = schema_record_frame_len(TYPE.len(), MD5.len(), DEF.len()).unwrap();
    let mut bytes = vec![0; size];
    assert_eq!(write_schema_record(&mut bytes, TYPE, MD5, DEF).unwrap(), size);
    let parsed = parse_schema_record(&bytes).unwrap();
    assert_eq!((parsed.data_type(), parsed.md5(), parsed.definition()), (TYPE, MD5, DEF));
}

#[test]
fn corrupt_footer_is_rejected() {
    let size = schema_record_frame_len(TYPE.len(), MD5.len(), DEF.len()).unwrap();
    let mut bytes = vec![0; size];
    write_schema_record(&mut bytes, TYPE, MD5, DEF).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert_eq!(parse_schema_record(&bytes), Err(SchemaRecordError::InvalidFrame));
}

#[test]
fn invalid_fields_are_rejected() {
    let mut bytes = vec![0; MAX_SCHEMA_RECORD_LEN];
    assert_eq!(write_schema_record(&mut bytes, "", MD5, DEF), Err(SchemaRecordError::WrongType));
    assert_eq!(write_schema_record(&mut bytes, TYPE, "bad", DEF), Err(SchemaRecordError::WrongMd5));
    assert_eq!(
        write_schema_record(&mut bytes, TYPE, MD5, ""),
        Err(SchemaRecordError::EmptyDefinition)
    );
}

#[test]
fn short_output_is_rejected() {
    let mut bytes = vec![0; 36];
    assert_eq!(
        write_schema_record(&mut bytes, TYPE, MD5, DEF),
        Err(SchemaRecordError::OversizedRecord)
    );
}

#[test]
fn only_lower_case_hex_md5_is_an_identity() {
    assert!(hypha_wire::is_valid_md5(MD5));
    let upper = "0123456789ABCDEF0123456789ABCDEF";
    assert!(!hypha_wire::is_valid_md5(upper));
    assert_eq!(hypha_wire::schema_hash_from_md5(upper), None);
    assert_eq!(hypha_wire::schema_hash_from_md5("0123456789abcdef"), None);
    assert_eq!(hypha_wire::schema_hash_from_md5("+a+a+a+a+a+a+a+a+a+a+a+a+a+a+a+a"), None);
    assert_eq!(
        hypha_wire::schema_hash_from_md5("0123456789abcdef0123456789abcdef"),
        Some(u64::from_le_bytes([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]))
    );
    let mut bytes = vec![0; MAX_SCHEMA_RECORD_LEN];
    assert_eq!(write_schema_record(&mut bytes, TYPE, upper, DEF), Err(SchemaRecordError::WrongMd5));
}
