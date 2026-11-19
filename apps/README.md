# Hypha Apps

Deployable applications built on [Hypha Core](https://github.com/sdig-vit-petrik/Hypha):

| Directory                 | Deliverable                                                        |
| ------------------------- | ------------------------------------------------------------------ |
| `apps/hypha_tools`      | CLI introspection, recording, playback, launch, and Foxglove tools |
| `apps/hypha_ros_bridge` | ROS 1/Noetic wire bridge                                           |

The applications are members of the repository's Cargo workspace. They use the
`hypha` crate from `crates/` by path, so they always build against the Core
revision of the same commit. They use Core's one shared C ABI library; they do
not embed their own transport implementation.

## Build

`./build.sh` in the repository root builds the library and the applications.
`./test.sh rust` runs their tests. To build
only the applications:

```sh
cargo build -p hypha_tools -p hypha_ros_bridge --release --locked
```

The CMake option `HYPHA_BUILD_APPS` (default `ON`) builds the tools with the SDK and
installs them to `bin/`. `./install.sh` in the repository root installs them:

```sh
./install.sh                    # /usr/local; builds as this user, sudo only to install
./install.sh ~/.local           # ~/.local/bin
```

The tools link the hypha runtime statically and need no `libhypha_ffi.so` at run
time; the C++ SDK and its applications do. Install these applications with `./install.sh` or the
CMake `apps` component. CI tests them but publishes no application image.
See [tools.md](../docs/tools.md) for the CLI reference.
