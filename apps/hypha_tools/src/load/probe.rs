//! The message every load topic carries: a send stamp, a sequence number and fill
//! bytes up to the topic's size, in ROS 1 serialization so that a recording of the run
//! decodes like any other.

/// The type of a probe message.
pub const PROBE_TYPE: &str = "hypha_load/Probe";
/// The definition of a probe message.
pub const PROBE_DEFINITION: &str = "uint64 sent_nanos\nuint64 sequence\nuint8[] fill\n";
/// Bytes before the fill: the stamp, the sequence and the fill's length.
pub const PROBE_HEADER_BYTES: usize = 8 + 8 + 4;

/// The MD5 of the probe definition, or `None` if it does not parse (a bug).
#[must_use]
pub fn probe_md5() -> Option<String> {
    hypha_node::sdk::md5::md5_from_definition(PROBE_TYPE, PROBE_DEFINITION)
}

/// A probe message of `bytes` bytes with zero stamp and sequence, or `None` for a size
/// below [`PROBE_HEADER_BYTES`] or a fill longer than its 32-bit length.
#[must_use]
pub fn new_probe(bytes: usize) -> Option<Vec<u8>> {
    let fill = bytes.checked_sub(PROBE_HEADER_BYTES)?;
    let fill_length = u32::try_from(fill).ok()?;
    let mut message = vec![0_u8; PROBE_HEADER_BYTES];
    let length = message.get_mut(16..PROBE_HEADER_BYTES)?;
    length.copy_from_slice(&fill_length.to_le_bytes());
    message.resize(bytes, 0x42);
    Some(message)
}

/// Writes the send stamp and the sequence into `message`, a probe from [`new_probe`].
/// Returns false for a buffer too short to be one.
pub fn write_header(message: &mut [u8], sent_nanos: u64, sequence: u64) -> bool {
    let Some(stamp) = message.get_mut(0..8) else {
        return false;
    };
    stamp.copy_from_slice(&sent_nanos.to_le_bytes());
    let Some(counter) = message.get_mut(8..16) else {
        return false;
    };
    counter.copy_from_slice(&sequence.to_le_bytes());
    true
}

/// The send stamp and the sequence of a probe, or `None` when `payload` is not one: too
/// short, or a fill length that does not match its size.
#[must_use]
pub fn read_header(payload: &[u8]) -> Option<(u64, u64)> {
    let stamp: [u8; 8] = payload.get(0..8)?.try_into().ok()?;
    let sequence: [u8; 8] = payload.get(8..16)?.try_into().ok()?;
    let length: [u8; 4] = payload.get(16..PROBE_HEADER_BYTES)?.try_into().ok()?;
    let fill = usize::try_from(u32::from_le_bytes(length)).ok()?;
    let matches = payload.len().checked_sub(PROBE_HEADER_BYTES) == Some(fill);
    matches.then(|| (u64::from_le_bytes(stamp), u64::from_le_bytes(sequence)))
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a test fails on the first refused step")]
mod tests {
    use super::*;

    #[test]
    fn a_probe_round_trips_its_stamp_and_sequence() {
        let mut probe = new_probe(64).expect("a probe of 64 bytes");
        assert_eq!(probe.len(), 64);
        assert!(write_header(&mut probe, 123_456_789, 42));
        assert_eq!(read_header(&probe), Some((123_456_789, 42)));
    }

    #[test]
    fn the_smallest_probe_has_an_empty_fill_and_a_smaller_one_is_refused() {
        let probe = new_probe(PROBE_HEADER_BYTES).expect("the smallest probe");
        assert_eq!(read_header(&probe), Some((0, 0)));
        assert_eq!(new_probe(PROBE_HEADER_BYTES - 1), None);
    }

    #[test]
    fn a_payload_that_is_no_probe_is_not_read() {
        let probe = new_probe(40).expect("a probe");
        assert_eq!(read_header(probe.get(..39).expect("shorter")), None, "fill length mismatch");
        assert_eq!(read_header(&[0; 10]), None, "too short");
    }

    #[test]
    fn the_probe_definition_has_an_md5() {
        let md5 = probe_md5().expect("the definition parses");
        assert!(hypha_node::sdk::is_valid_md5(&md5), "{md5}");
    }
}
