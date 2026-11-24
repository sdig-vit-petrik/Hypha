# Getting started

## Build

```sh
./build.sh
```

That builds the shared library `libhypha_ffi.so`, the C++ SDK package
(`hypha-cxx`), its tests, the CLI tools and the examples, in Release.

The library is versioned by the root file `abi.revision`. Cargo writes the unversioned
name. The CMake build directory holds a link with the versioned name:
`libhypha_ffi.so.<revision>` on Linux (also its SONAME), and `libhypha_ffi.<revision>.dylib`
on macOS (install name `@rpath/libhypha_ffi.<revision>.dylib`).

Run the tests with `./test.sh` (clippy, Rust tests, ctest, installed SDK checks),
and the style checks with `./check.sh`. Every test runs in a bus domain of its own
under the test root `t_`, so the gate runs beside a hypha system on the same host;
`test.sh` removes only the test domains that no live process uses
(`./clean-shm.sh --domain t_`).

For `sccache`, set `RUSTC_WRAPPER=sccache` and `CMAKE_CXX_COMPILER_LAUNCHER=sccache`
in your environment.

The Python tools of the gate (`check.sh`, `scripts/format.sh`) need the development requirements:

```sh
python3 -m pip install -r requirements-dev.txt
```

## Use the SDK

Install Core into a prefix, then build a consumer against that prefix:

```sh
cmake --install build --prefix /path/to/hypha-sdk
cmake -S test/package_consumer -B build/consumer \
  -DCMAKE_PREFIX_PATH=/path/to/hypha-sdk
cmake --build build/consumer
```

The install prefix holds only the versioned library. Rebuild every consumer against
an install whose library revision differs from the one the consumer was built with.

A test that uses the bus registers with `hypha_add_test()`, which the SDK installs beside
`hypha_generate_messages()`. It runs the test in a bus domain of its own,
`t_<pid of ctest>_<n>`, so parallel tests, the tests of another build tree and a system
running on the host never share a node name, topic or `/clock`:

```cmake
enable_testing()
add_executable(my_node_test test/my_node_test.cpp)
target_link_libraries(my_node_test PRIVATE hypha::sdk GTest::gtest_main)
hypha_add_test(my_node_test)
# A script or a command line: COMMAND, and WORKING_DIRECTORY as for add_test().
hypha_add_test(my_flight COMMAND bash "${CMAKE_CURRENT_SOURCE_DIR}/fly.sh" my_node_test)
```

The [examples](../examples/README.md) are runnable nodes, such as `speed`. The [apps](../apps/README.md) are `hypha topic` and the other
CLI tools. The CMake options `HYPHA_BUILD_APPS` and `HYPHA_BUILD_EXAMPLES` (both
`ON` by default) add them to the build and to `cmake --install`. `./install.sh`
installs the SDK and the CLI tools without the examples. `examples/check.sh`
builds the examples against an installed SDK and flies the MRS scenarios; `./test.sh
cpp` runs it, so it needs Eigen 3 (in the `hypha-builder` image).

## If something goes wrong

`hypha ABI revision mismatch: SDK expects N, library reports M` means that the
binary and the loaded library come from different builds. The SDK refuses to
start, so no node runs. Rebuild the binary against the installed SDK, and check
that the loader finds the versioned library of that SDK. See
[cpp-api.md](cpp-api.md) section 2.1.

A `PortError::Backend(...)` right after you change a transport capacity or
switch versions is usually stale shared memory, not a code fault. The old
segment keeps the old limits for as long as anything references it.
A node that starts reclaims the resources of the dead runs of its own name
and of every dead CLI tool node (`_hyphacli_<tool>`). If the dead run of its own
name cannot be reclaimed, the node does not start. Any other dead node
keeps its resources until a node of its name starts again. A segment whose
owning process is alive but stuck, or a dead node whose name does not
return, needs the manual wipe:

```sh
./clean-shm.sh --domain t_   # the test domains nobody uses; kills nothing
./clean-shm.sh               # every domain; refuses if a process still holds a segment
./clean-shm.sh --force       # every domain; kills the holders first
```

## Writing your own node

See [cpp-api.md](cpp-api.md) for the C++ API. For Rust, the API
documentation is the crate documentation:

```sh
cargo doc --open -p hypha
```

Copy `speed` from `examples/` as the starting layout for a C++ node package; its
`CMakeLists.txt` shows the `find_package(hypha)` wiring.
