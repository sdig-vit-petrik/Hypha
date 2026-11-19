//! A test-only MCAP reader for golden-sample tests: the Rust half of the C++
//! `hypha::testing::McapReplay`.
//!
//! [`Reader::open`] reads one MCAP file into memory and checks it whole; payloads are
//! returned as stored. The C API (`hypha_mcap_*`) is a thin layer over [`Reader`], built
//! as a static library for the C++ testing component, not part of `libhypha_ffi`.

mod reader;

#[expect(unsafe_code, reason = "the C boundary turns raw pointers from C into Rust values")]
mod ffi;

pub use ffi::{
    HyphaMcapBytes, HyphaMcapChannel, HyphaMcapMessage, HyphaMcapReader, HyphaMcapStatus,
    hypha_mcap_channel, hypha_mcap_channel_count, hypha_mcap_close, hypha_mcap_last_error,
    hypha_mcap_next, hypha_mcap_open, hypha_mcap_rewind, hypha_mcap_select,
};
pub use reader::{Channel, Error, MAX_FILE_BYTES, MD5_METADATA_KEY, Message, Reader};
