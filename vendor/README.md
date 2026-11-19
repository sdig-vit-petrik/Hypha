# Vendored crates

Third-party crates hypha patches. Each is the published crate unchanged except for the
lines marked `hypha patch`. `Cargo.toml` points `[patch.crates-io]` at them. Update a
vendored crate together with its upstream version in `Cargo.lock`, and re-apply the patch.

## iceoryx2-cal 0.10.0

`src/resizable_shared_memory/dynamic.rs`: `MAX_NUMBER_OF_REALLOCATIONS` is 32 instead of
`SegmentId::max_segment_id()` (255).

A growable publisher (`PayloadCapacity::Growable`, `AllocationStrategy::PowerOfTwo`) and
every connection to it keep bookkeeping of 8 bytes per pool sample for each segment the
publisher could ever create. At 255 segments that was the largest share of a topic's shared
memory and of a node's heap for small messages. A growth creates a segment at the next
power of two that fits the message, so 32 growths cover any message up to 2^32 times the
first slot size. Past the limit a loan fails with `AllocationError::OutOfMemory`, which
hypha reports as a refused send.

Remove the patch when iceoryx2 makes the limit configurable.
