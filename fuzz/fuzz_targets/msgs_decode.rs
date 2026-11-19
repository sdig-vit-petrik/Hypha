//! Two generated decoders of `hypha_msgs` with nested arrays on arbitrary bytes.
//! `DiagnosticArray` nests an array of `DiagnosticStatus`, each with an array of
//! `KeyValue`; `PointCloud2` has an array of `PointField` and a byte array.
#![no_main]

use hypha_core::message::{DecodeOwned, EncodeInto};
use hypha_msgs::diagnostic_msgs::DiagnosticArray;
use hypha_msgs::sensor_msgs::PointCloud2;
use libfuzzer_sys::fuzz_target;

fn check<T>(bytes: &[u8], message: &T, capacities: &[usize])
where
    T: EncodeInto + DecodeOwned + PartialEq + std::fmt::Debug,
{
    assert!(message.encoded_len() <= bytes.len(), "decoded more than the bytes hold");
    // Every element takes at least one byte here, so a capacity past the bytes was
    // reserved from the wire alone.
    for capacity in capacities {
        assert!(*capacity <= bytes.len(), "capacity {capacity} from {} bytes", bytes.len());
    }
    let mut again = vec![0_u8; message.encoded_len()];
    let written = message.encode_into(&mut again).expect("the sized buffer is refused");
    assert_eq!(written, again.len());
    let decoded = T::decode_from(&again).expect("a written message is refused");
    assert_eq!(&decoded, message);
}

fuzz_target!(|bytes: &[u8]| {
    if let Ok(message) = DiagnosticArray::decode_from(bytes) {
        let mut capacities = vec![message.status.capacity()];
        capacities.extend(message.status.iter().map(|status| status.values.capacity()));
        check(bytes, &message, &capacities);
    }
    if let Ok(message) = PointCloud2::decode_from(bytes) {
        check(bytes, &message, &[message.fields.capacity(), message.data.capacity()]);
    }
});
