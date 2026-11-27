# Vendored crates

Third-party crates hypha patches. Each is the published crate unchanged except for the
lines marked `hypha patch`. `Cargo.toml` and `fuzz/Cargo.toml` both point
`[patch.crates-io]` at them (a workspace without the lines silently builds the registry
crate). `crates.txt` pins each crate's version and the sha256 of its tarball;
`scripts/check-vendor.sh`, run by `check.sh`, compares every directory with that tarball and
refuses a change no `hypha patch` line marks.

To re-vendor a crate, here iceoryx2 from `OLD` to `NEW`, from the repository root:

```bash
OLD=0.10.1 NEW=0.10.2 CRATE=iceoryx2 WORK=$(mktemp -d)
for v in "$OLD" "$NEW"; do
  curl -sSfL -o "$WORK/$v.crate" "https://static.crates.io/crates/$CRATE/$CRATE-$v.crate"
  mkdir "$WORK/$v" && tar -xzf "$WORK/$v.crate" -C "$WORK/$v" --strip-components 1
done
shasum -a 256 "$WORK/$NEW.crate"                       # the new line of crates.txt
# The hypha patch: the vendored tree against its own release.
diff -ru -x .cargo-ok -x .cargo_vcs_info.json -x Cargo.toml.orig \
  "$WORK/$OLD" "vendor/$CRATE" > "$WORK/hypha.patch" || true
rm -rf "vendor/$CRATE" && cp -R "$WORK/$NEW" "vendor/$CRATE"
rm -f "vendor/$CRATE"/{.cargo-ok,.cargo_vcs_info.json,Cargo.toml.orig}
patch -d "vendor/$CRATE" -p2 < "$WORK/hypha.patch"    # resolve any reject by hand
# Edit crates.txt (version, sha256) and the iceoryx2 version in Cargo.toml files, then:
cargo update -p "$CRATE" --precise "$NEW"
cargo update --manifest-path fuzz/Cargo.toml -p "$CRATE" --precise "$NEW"
./scripts/check-vendor.sh
```

The version in `Cargo.lock` (main and `fuzz/`) and in `crates.txt` move together.

## iceoryx2 0.10.1

`src/sample_mut.rs`: `SampleMut<_, [T], _>::truncate(n)` shortens a loaned slice to its
first `n` elements before the send. It sets the element count of the sample header, as
iceoryx2's own flatbuffer path does after the payload is written (`SampleMutUninit::
__internal_finish_serialized`); the chunk keeps its size. hypha uses it for a typed loan whose
message is shorter than the bytes it reserved (`hypha_loan_shrink`, issue #255).

Remove the patch when iceoryx2 can shorten a loaned slice.

## iceoryx2-cal 0.10.1

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
