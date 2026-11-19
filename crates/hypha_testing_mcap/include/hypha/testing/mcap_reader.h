/* The MCAP test reader of hypha's C++ testing component (crate hypha_testing_mcap,
 * static library libhypha_testing_mcap.a). A private header: not part of the hypha
 * C ABI, and not in libhypha_ffi.
 *
 * Rules for every function: a reader is used by one thread at a time (different
 * readers may be used on different threads at once). No function keeps a pointer it
 * was given after it returns. Text is UTF-8, a pointer and a length, not
 * NUL-terminated. A Rust panic below this API aborts the process. */

#ifndef HYPHA_TESTING_MCAP_READER_H
#define HYPHA_TESTING_MCAP_READER_H

#include <stddef.h>
#include <stdint.h>

// The result of a `hypha_mcap_*` call.
typedef enum HyphaMcapStatus {
  // The call did what it says.
  HYPHA_MCAP_STATUS_OK = 0,
  // `hypha_mcap_next` only: no selected message is left. Nothing was written.
  HYPHA_MCAP_STATUS_END = 1,
  // The call was refused; `hypha_mcap_last_error` names why (no text for a null reader).
  HYPHA_MCAP_STATUS_ERROR = 2,
} HyphaMcapStatus;

// An open MCAP file. Opaque: made by `hypha_mcap_open`, freed by `hypha_mcap_close`.
typedef struct HyphaMcapReader HyphaMcapReader;

// Borrowed bytes: `len` bytes at `data`. `data` is null only when `len` is 0.
typedef struct HyphaMcapBytes {
  // The first byte.
  const uint8_t *data;
  // The number of bytes.
  size_t len;
} HyphaMcapBytes;

// One channel of an open file. Every text is borrowed from the reader and stays
// valid until `hypha_mcap_close`.
typedef struct HyphaMcapChannel {
  // The topic.
  struct HyphaMcapBytes topic;
  // The schema name (`geometry_msgs/Point`); empty for a channel without a schema.
  struct HyphaMcapBytes type_name;
  // The schema encoding: `ros1msg` for a ROS 1 message; empty without a schema.
  struct HyphaMcapBytes schema_encoding;
  // The message encoding (`ros1` for a ROS 1 message).
  struct HyphaMcapBytes message_encoding;
  // The ROS 1 MD5 from the channel metadata key `md5sum`; empty when not recorded.
  struct HyphaMcapBytes md5;
  // The number of messages of the channel in the file.
  uint64_t message_count;
} HyphaMcapChannel;

// One message of a selected topic.
typedef struct HyphaMcapMessage {
  // The index of its channel, as `hypha_mcap_channel` takes it.
  size_t channel_index;
  // Its log time in nanoseconds.
  uint64_t log_time_ns;
  // Its publish time in nanoseconds.
  uint64_t publish_time_ns;
  // Its payload as stored, borrowed from the reader. Valid until the following
  // `hypha_mcap_next`, `hypha_mcap_rewind` or `hypha_mcap_close` on the same reader.
  struct HyphaMcapBytes payload;
} HyphaMcapMessage;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// Open the MCAP file at the `path_len` bytes of `path` (not NUL-terminated), read it
// whole into memory and check it.
//
// `HYPHA_MCAP_STATUS_OK`: writes the new reader to `*out_reader`; the caller owns it and
// frees it with `hypha_mcap_close`. No topic is selected.
//
// `HYPHA_MCAP_STATUS_ERROR`: writes null to `*out_reader` (when not null) and reports
// the error text by the size-query protocol: its length to `*error_len` (when not null),
// its bytes to `error` only when `error_capacity` holds them all. Refused: a null `path`
// or `out_reader`, an unreadable file, a file above 1 GiB, and any malformed or
// unsupported content (lz4 and zstd chunks are supported).
//
// # Safety
// `path` must be null or point to `path_len` readable bytes. `out_reader` must be null
// or point to writable storage for one pointer. `error` must be null or point to
// `error_capacity` writable bytes. `error_len` must be null or point to a writable
// `size_t`.
enum HyphaMcapStatus hypha_mcap_open(const uint8_t *path,
                                     size_t path_len,
                                     struct HyphaMcapReader **out_reader,
                                     uint8_t *error,
                                     size_t error_capacity,
                                     size_t *error_len);

// The number of channels of the file; 0 for a null reader.
//
// # Safety
// `reader` must be null or an open reader that no other thread uses during the call.
size_t hypha_mcap_channel_count(const struct HyphaMcapReader *reader);

// Describe channel `index` (`0 <= index < hypha_mcap_channel_count`) into `*out`. Its
// texts are borrowed from the reader and stay valid until `hypha_mcap_close`.
//
// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text), a null `out` or an
// `index` out of range; nothing is written to `*out` then.
//
// # Safety
// `reader` must be null or an open reader that no other thread uses during the call.
// `out` must be null or point to writable storage for one `HyphaMcapChannel`.
enum HyphaMcapStatus hypha_mcap_channel(struct HyphaMcapReader *reader,
                                        size_t index,
                                        struct HyphaMcapChannel *out);

// Add every channel whose topic is the `topic_len` bytes of `topic` (not
// NUL-terminated) to the selection that `hypha_mcap_next` returns. Does not move the cursor.
//
// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text), a null `topic`, a topic
// that is not UTF-8, or a topic no channel has; the selection is unchanged then.
//
// # Safety
// `reader` must be null or an open reader that no other thread uses during the call.
// `topic` must be null or point to `topic_len` readable bytes.
enum HyphaMcapStatus hypha_mcap_select(struct HyphaMcapReader *reader,
                                       const uint8_t *topic,
                                       size_t topic_len);

// Move the cursor back to the first message; the selection is kept. Ends the validity
// of the last message's payload.
//
// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text).
//
// # Safety
// `reader` must be null or an open reader that no other thread uses during the call.
enum HyphaMcapStatus hypha_mcap_rewind(struct HyphaMcapReader *reader);

// Write the next message of a selected topic to `*out`, in non-decreasing log time,
// ties in file order. The payload is valid until the following `hypha_mcap_next`,
// `hypha_mcap_rewind` or `hypha_mcap_close` on this reader.
//
// `HYPHA_MCAP_STATUS_END` when no selected message is left (at once with no topic
// selected), until `hypha_mcap_rewind`; nothing is written then.
// `HYPHA_MCAP_STATUS_ERROR` for a null reader (no error text) or a null `out`.
//
// # Safety
// `reader` must be null or an open reader that no other thread uses during the call.
// `out` must be null or point to writable storage for one `HyphaMcapMessage`.
enum HyphaMcapStatus hypha_mcap_next(struct HyphaMcapReader *reader,
                                     struct HyphaMcapMessage *out);

// The text of the last call on this reader that returned `HYPHA_MCAP_STATUS_ERROR`,
// borrowed from the reader: empty when the last call succeeded or returned
// `HYPHA_MCAP_STATUS_END`, and `{ NULL, 0 }` for a null reader. Valid until the next
// call on this reader other than `hypha_mcap_last_error` and `hypha_mcap_channel_count`.
//
// # Safety
// `reader` must be null or an open reader that no other thread uses during the call.
struct HyphaMcapBytes hypha_mcap_last_error(const struct HyphaMcapReader *reader);

// Free a reader and everything borrowed from it. A null reader does nothing.
//
// # Safety
// `reader` must be null or an open reader that no other thread uses during the call.
// It must not be used afterwards.
void hypha_mcap_close(struct HyphaMcapReader *reader);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* HYPHA_TESTING_MCAP_READER_H */
