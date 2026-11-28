# Getting started

## Build and test

```sh
./build.sh        # Release: libhypha_ffi, the C++ SDK, CLI tools, examples, tests
./test.sh         # clippy, Rust tests, ctest, the installed-SDK check
./check.sh        # style, generated files, size and dependency rules
```

`check.sh` and `scripts/format.sh` need `python3 -m pip install -r requirements-dev.txt`.
For a compiler cache, set `RUSTC_WRAPPER=sccache` and `CMAKE_CXX_COMPILER_LAUNCHER=sccache`.

Each test runs in its own bus domain under `t_`, so the tests can run beside a live hypha
system on the same host. `test.sh` then removes the test domains that no process uses.

The library name carries the ABI revision from the file `abi.revision`:
`libhypha_ffi.so.<revision>` on Linux (also its SONAME), `libhypha_ffi.<revision>.dylib` on
macOS. A binary runs only with the revision it was built against.

## Use the SDK

Install the SDK into a prefix, then build against it:

```sh
cmake --install build --prefix /path/to/hypha-sdk
cmake -S test/package_consumer -B build/consumer -DCMAKE_PREFIX_PATH=/path/to/hypha-sdk
cmake --build build/consumer
```

`./install.sh [prefix]` installs the SDK and the CLI tools without the examples. The CMake
options `HYPHA_BUILD_APPS` and `HYPHA_BUILD_EXAMPLES` (both `ON`) add the tools and the
examples to the build and the install.

Add a node with `hypha_add_node()`. It links `hypha::sdk` and sets the RPATH, so the
installed binary finds `libhypha_ffi`:

```cmake
find_package(hypha CONFIG REQUIRED)
hypha_add_node(my_node src/main.cpp)
install(TARGETS my_node RUNTIME DESTINATION "${CMAKE_INSTALL_BINDIR}")
```

Add a test that uses the bus with `hypha_add_test()`. It gives the test its own bus domain,
so parallel tests and a running system never share a node name, a topic or `/clock`:

```cmake
enable_testing()
find_package(GTest REQUIRED)
add_executable(my_node_test test/my_node_test.cpp)
target_link_libraries(my_node_test PRIVATE hypha::sdk GTest::gtest_main)
hypha_add_test(my_node_test)
hypha_add_test(my_flight COMMAND bash "${CMAKE_CURRENT_SOURCE_DIR}/fly.sh" my_node_test)
```

Start a new node package from `examples/speed`; its `CMakeLists.txt` shows the wiring. The
C++ API is in [cpp-api.md](cpp-api.md). The Rust SDK is documented in its crate:
`cargo doc --open -p hypha_node`.

`examples/check.sh` builds the examples against an installed SDK and runs the MRS scenarios.
`./test.sh cpp` runs it, so it needs Eigen 3 (in the `hypha-builder` image).

## When something goes wrong

**`hypha ABI revision mismatch: SDK expects N, library reports M`.** The binary and the
library come from different builds, and no node starts. Rebuild the binary against the
installed SDK and check that the loader finds that SDK's library ([cpp-api.md](cpp-api.md)
§2.1).

**`PortError::Backend(...)` after you change a transport capacity or a version.** Usually
stale shared memory: an old segment keeps its old limits while anything uses it.

- A starting node reclaims the dead runs of its own name and of every dead CLI tool node
  (`_hyphacli_<tool>`). If it cannot reclaim its own name, it does not start.
- Any other dead node keeps its resources until a node of its name starts again.
- For a stuck process or a dead node whose name never returns, stop the domain and clean it.
  `hypha clean` kills nothing ([tools.md](tools.md)):

```sh
hypha clean --domain t_   # the test domains no process uses
hypha clean               # the domain of HYPHA_DOMAIN; refused while one of its nodes lives
```

In the repository, `./clean-shm.sh` wipes every domain of the host and refuses while a
process holds a segment; `./clean-shm.sh --force` kills those processes first.
