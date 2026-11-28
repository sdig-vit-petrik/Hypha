# Hypha

[![ci](https://github.com/sdig-vit-petrik/Hypha/actions/workflows/ci.yml/badge.svg?branch=develop)](https://github.com/sdig-vit-petrik/Hypha/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/sdig-vit-petrik/Hypha)](https://github.com/sdig-vit-petrik/Hypha/releases)
[![license](https://img.shields.io/github/license/sdig-vit-petrik/Hypha)](LICENSE)

**Single-host shared-memory middleware for C++23 robotics nodes.**

Hypha replaces the local ROS 1 data plane on a single robot with zero-copy `iceoryx2` shared memory, ROS 1-compatible message generation, explicit input freshness/supervision, and a small set of CLI tools — with no central master. The core runtime is written in Rust and delivered as a single shared library (`libhypha_ffi`) behind a C++23 SDK.

![C++ and Rust nodes share one iceoryx2 shared-memory segment on one host with no broker; bridge processes connect it to ROS 1 and Foxglove](docs/hypha.svg)

## Scope

Hypha covers local communication on one machine:
- shared-memory publish/subscribe (with zero-copy loans and latched topics) and typed `.srv` services;
- periodic and event-triggered C++23 cycle nodes with built-in staleness and stagnation checks;
- ROS 1 `.msg`/`.srv` code generation (`hypha_msggen`) and a straightforward ROS 1 C++ migration path;
- CLI tools (`hypha topic`, `top`, `record`, `play`, `launch`, `intercept`, `clean`, `load`) and separate bridge processes for ROS 1 and Foxglove.

Multi-host routing, complete ROS graph compatibility, actions, lifecycle nodes, and plugin ecosystems are out of scope. Network-facing code lives in separate bridge processes.

---

## Prerequisites

- **OS:** Ubuntu 26.04 (amd64 or arm64) is the supported target; macOS works for local development.
- **C++:** GCC or Clang with **C++23** support, **CMake ≥ 3.24**.
- **Rust:** stable toolchain (see `rust-toolchain.toml`) — only needed to build Hypha from source; application code is plain C++23.

---

## Install (C++ SDK + CLI tools)

Build and install the C++ SDK (`hypha::sdk`, standard message headers, `hypha_generate_messages()`) and the `hypha` CLI tools:

```bash
git clone https://github.com/sdig-vit-petrik/Hypha.git
cd Hypha
./install.sh            # installs into /usr/local (pass a custom prefix as $1)
```

Or pull Hypha directly into your CMake project with `FetchContent` (no global install needed):

```cmake
include(FetchContent)
FetchContent_Declare(hypha GIT_REPOSITORY https://github.com/sdig-vit-petrik/Hypha.git GIT_TAG <tag-or-commit>)
set(HYPHA_BUILD_TESTS OFF)
set(HYPHA_BUILD_APPS OFF)
set(HYPHA_BUILD_EXAMPLES OFF)
FetchContent_MakeAvailable(hypha)
```

---

## Write a C++ Node

### `CMakeLists.txt`

```cmake
cmake_minimum_required(VERSION 3.24)
project(my_robot_node LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 23)

find_package(hypha REQUIRED)

add_executable(doubler src/doubler.cpp)
target_link_libraries(doubler PRIVATE hypha::sdk)
```

For custom `.msg`/`.srv` packages, call `hypha_generate_messages(my_msgs ...)` and link the generated target.

### `src/doubler.cpp`

```cpp
#include <hypha/hypha.h>
#include <hypha_log/hypha_log.h>
#include <std_msgs/Float64.h>

HYPHA_LOG_MODULE_INIT("doubler")

struct Doubler {
  hypha::Publisher<std_msgs::Float64>* out;

  void operator()(const hypha::Subscriber<std_msgs::Float64>::ConstPtr& input) const {
    std_msgs::Float64 msg;
    msg.data = input->data * 2.0;
    if (!this->out->publish(msg)) {
      HYPHA_LOG_WARN_THROTTLE(1.0, "publish failed: {}",
                              this->out->lastPublishStatus().name());
    }
  }
};

int main() {
  hypha::Node node("doubler");
  auto out = node.advertise<std_msgs::Float64>(hypha::Topic{"/output/data"});
  const Doubler doubler{&out};
  auto in = node.subscribe<std_msgs::Float64>(hypha::Topic{"/sensors/data"}, doubler);

  return node.run().ok() ? 0 : 1; // runs until SIGINT / SIGTERM
}
```

For periodic or triggered nodes with TOML configuration and automatic input freshness checks, inherit from `hypha::CycleNode` — see [docs/cpp-api.md](docs/cpp-api.md) and [examples/](examples/README.md).

Inspect a running system with the CLI:

```bash
hypha top
hypha topic list
hypha topic echo /output/data
```

---

## Building and Testing This Repository

```bash
./build.sh [--clean]            # Release build: library, C++ SDK, CLI tools, examples, tests
./test.sh [--clean] [rust|cpp]  # Run clippy + Rust tests and/or C++ tests + installed SDK check
./check.sh                      # Formatting, links, generated headers, dependency & size gates
./scripts/format.sh             # Format Rust, C++, and Python in place
```

Docker builder/runtime images are defined in `docker/Dockerfile` (`./docker/build.sh builder|runtime`; run any gate inside the builder image with `scripts/in-image.sh ./check.sh`). Qualification tools (`scripts/coverage.sh`, `miri.sh`, `fuzz.sh`, `sanitize.sh`, `qualify.sh`) are documented in [docs/getting-started.md](docs/getting-started.md).

---

## Documentation

| Guide | What it covers |
|---|---|
| [Getting Started](docs/getting-started.md) | Building, installing, CMake integration, troubleshooting shared memory |
| [C++ API](docs/cpp-api.md) | `CycleNode`, ports, freshness/supervision, loans, timers, parameters, TF |
| [ROS 1 Migration Guide](docs/ros1-migration-guide.md) | Porting ROS 1 C++ nodes to Hypha |
| [Execution Model](docs/execution-model.md) | `WaitSet`, cycles, callbacks, timers, simulated time |
| [CLI Tools](docs/tools.md) | `hypha topic`, `top`, `record`, `play`, `launch`, `intercept`, `clean`, `load` |
| [Architecture](docs/architecture.md) & [C ABI](docs/interface-specification.md) | Layering, `libhypha_ffi` contract, wire frame format |
| [Examples](examples/README.md) & [Apps](apps/README.md) | Runnable C++ nodes, CLI tools, ROS 1 bridge |
| [Full Documentation Index](docs/README.md) | Requirements, failure analysis, safety manual, release policy |

---

## Contributing & License

See [CONTRIBUTING.md](CONTRIBUTING.md). Licensed under Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)).
