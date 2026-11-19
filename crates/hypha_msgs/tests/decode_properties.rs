//! Property tests of the generated decoders: no panic on any bytes, no reservation past
//! the bytes, and what is written reads back.

use hypha_core::message::{DecodeOwned, EncodeInto};
use hypha_msgs::diagnostic_msgs::{DiagnosticArray, DiagnosticStatus, KeyValue};
use hypha_msgs::sensor_msgs::{PointCloud2, PointField};
use hypha_msgs::std_msgs::Header;
use hypha_wire::ros_time::RosTime;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Shrunk failures are kept here, relative to the crate directory, and replayed first.
const REGRESSIONS: &str = "proptest-regressions/decode_properties.txt";

/// Miri interprets every step: a handful of cases still walks each code path.
const MIRI_CASES: u32 = 4;

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases: if cfg!(miri) { MIRI_CASES } else { cases },
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(REGRESSIONS))),
        ..ProptestConfig::default()
    }
}

fn encoded<T: EncodeInto>(message: &T) -> Result<Vec<u8>, TestCaseError> {
    let mut buffer = vec![0_u8; message.encoded_len()];
    let written = message
        .encode_into(&mut buffer)
        .map_err(|error| TestCaseError::fail(format!("the sized buffer is refused: {error}")))?;
    prop_assert_eq!(written, buffer.len());
    Ok(buffer)
}

fn header() -> impl Strategy<Value = Header> {
    (any::<u32>(), any::<u32>(), any::<u32>(), "\\PC{0,8}").prop_map(
        |(seq, secs, nsecs, frame_id)| Header { seq, stamp: RosTime { secs, nsecs }, frame_id },
    )
}

fn diagnostic_array() -> impl Strategy<Value = DiagnosticArray> {
    let key_value = ("\\PC{0,6}", "\\PC{0,6}").prop_map(|(key, value)| KeyValue { key, value });
    let status = (any::<i8>(), "\\PC{0,6}", "\\PC{0,6}", "\\PC{0,6}", vec(key_value, 0..4))
        .prop_map(|(level, name, message, hardware_id, values)| DiagnosticStatus {
            level,
            name,
            message,
            hardware_id,
            values,
        });
    (header(), vec(status, 0..4)).prop_map(|(header, status)| DiagnosticArray { header, status })
}

fn point_cloud() -> impl Strategy<Value = PointCloud2> {
    let field = ("\\PC{0,6}", any::<u32>(), any::<u8>(), any::<u32>())
        .prop_map(|(name, offset, datatype, count)| PointField { name, offset, datatype, count });
    (header(), any::<(u32, u32, bool, u32, u32, bool)>(), vec(field, 0..4), vec(any::<u8>(), 0..32))
        .prop_map(
            |(
                header,
                (height, width, is_bigendian, point_step, row_step, is_dense),
                fields,
                data,
            )| {
                PointCloud2 {
                    header,
                    height,
                    width,
                    fields,
                    is_bigendian,
                    point_step,
                    row_step,
                    data,
                    is_dense,
                }
            },
        )
}

/// What a decoded message must satisfy against the `bytes` it came from.
fn check_decoded<T>(bytes: &[u8], message: &T, capacities: &[usize]) -> Result<(), TestCaseError>
where
    T: EncodeInto + DecodeOwned + PartialEq + std::fmt::Debug,
{
    prop_assert!(message.encoded_len() <= bytes.len());
    // Every element takes at least one byte, so a count past the bytes came from the wire.
    for capacity in capacities {
        prop_assert!(*capacity <= bytes.len(), "capacity {} from {} bytes", capacity, bytes.len());
    }
    let again = encoded(message)?;
    let decoded = T::decode_from(&again)
        .map_err(|error| TestCaseError::fail(format!("a written message is refused: {error}")))?;
    prop_assert_eq!(&decoded, message);
    Ok(())
}

fn array_capacities(message: &DiagnosticArray) -> Vec<usize> {
    let mut capacities = vec![message.status.capacity()];
    capacities.extend(message.status.iter().map(|status| status.values.capacity()));
    capacities
}

fn cloud_capacities(message: &PointCloud2) -> Vec<usize> {
    vec![message.fields.capacity(), message.data.capacity()]
}

/// `bytes` with byte `at` changed by `delta`, or the 4-byte count at `at` set to
/// `count` when `count` is given.
fn corrupt(mut bytes: Vec<u8>, at_permille: usize, delta: u8, count: Option<u32>) -> Vec<u8> {
    let at = bytes.len().saturating_mul(at_permille) / 1000;
    match count {
        Some(count) => {
            if let Some(slot) = bytes.get_mut(at..at.saturating_add(4)) {
                slot.copy_from_slice(&count.to_le_bytes());
            }
        }
        None => {
            if let Some(byte) = bytes.get_mut(at) {
                *byte = byte.wrapping_add(delta);
            }
        }
    }
    bytes
}

fn count() -> impl Strategy<Value = Option<u32>> {
    proptest::option::of(prop_oneof![Just(u32::MAX), Just(0x8000_0000), 0..64_u32])
}

proptest! {
    #![proptest_config(config(512))]

    #[test]
    fn a_diagnostic_array_reads_back(message in diagnostic_array()) {
        let bytes = encoded(&message)?;
        let decoded = DiagnosticArray::decode_from(&bytes)
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        prop_assert_eq!(decoded, message);
    }

    #[test]
    fn a_point_cloud_reads_back(message in point_cloud()) {
        let bytes = encoded(&message)?;
        let decoded = PointCloud2::decode_from(&bytes)
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        prop_assert_eq!(decoded, message);
    }

    #[test]
    fn a_changed_diagnostic_array_decodes_within_its_bytes_or_is_refused(
        message in diagnostic_array(),
        at_permille in 0..1000_usize,
        delta in 1..=255_u8,
        count in count(),
        cut_permille in 0..=1000_usize,
    ) {
        let bytes = corrupt(encoded(&message)?, at_permille, delta, count);
        let cut = bytes.len().saturating_mul(cut_permille) / 1000;
        let bytes = bytes.get(..cut).unwrap_or_default();
        if let Ok(decoded) = DiagnosticArray::decode_from(bytes) {
            check_decoded(bytes, &decoded, &array_capacities(&decoded))?;
        }
    }

    #[test]
    fn a_changed_point_cloud_decodes_within_its_bytes_or_is_refused(
        message in point_cloud(),
        at_permille in 0..1000_usize,
        delta in 1..=255_u8,
        count in count(),
        cut_permille in 0..=1000_usize,
    ) {
        let bytes = corrupt(encoded(&message)?, at_permille, delta, count);
        let cut = bytes.len().saturating_mul(cut_permille) / 1000;
        let bytes = bytes.get(..cut).unwrap_or_default();
        if let Ok(decoded) = PointCloud2::decode_from(bytes) {
            check_decoded(bytes, &decoded, &cloud_capacities(&decoded))?;
        }
    }

    #[test]
    fn any_bytes_decode_within_their_length_or_are_refused(bytes in vec(any::<u8>(), 0..160)) {
        if let Ok(decoded) = DiagnosticArray::decode_from(&bytes) {
            check_decoded(&bytes, &decoded, &array_capacities(&decoded))?;
        }
        if let Ok(decoded) = PointCloud2::decode_from(&bytes) {
            check_decoded(&bytes, &decoded, &cloud_capacities(&decoded))?;
        }
    }
}
