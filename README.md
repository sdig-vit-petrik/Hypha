# Hypha

[![ci](https://github.com/sdig-vit-petrik/Hypha/actions/workflows/ci.yml/badge.svg?branch=develop)](https://github.com/sdig-vit-petrik/Hypha/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/sdig-vit-petrik/Hypha)](https://github.com/sdig-vit-petrik/Hypha/releases)
[![license](https://img.shields.io/github/license/sdig-vit-petrik/Hypha)](LICENSE)

Shared-memory middleware for C++23 robotics nodes on one host.

Hypha replaces the local ROS 1 data plane of a robot. Nodes talk through `iceoryx2` shared
memory, with no master process. Messages are generated from ROS 1 `.msg` and `.srv` files.
Each input has a freshness rule. The runtime is Rust, shipped as one shared library
(`libhypha_ffi`) under a C++23 SDK.

![C++ and Rust nodes share one iceoryx2 shared-memory segment on one host with no broker; bridge processes connect it to ROS 1 and Foxglove](docs/hypha.svg)

## What it does

- Publish and subscribe over shared memory, with zero-copy loans and latched topics.
- Typed services from `.srv` files.
- Periodic and triggered cycle nodes that check each input for staleness and a stuck sender.
- Code generation from ROS 1 `.msg` and `.srv` files (`hypha_msggen`), and a guide for
  porting ROS 1 C++ nodes.
- CLI tools: `hypha topic`, `top`, `record`, `play`, `launch`, `intercept`, `clean`, `load`.
- Bridge processes for ROS 1 and Foxglove.

Out of scope: more than one host, the full ROS graph API, actions and lifecycle nodes.
Network code runs only in the bridge processes.

## Requirements

- Ubuntu 26.04 (amd64 or arm64). macOS works for development.
- GCC or Clang with C++23, and CMake 3.24 or newer.
- A Rust toolchain (`rust-toolchain.toml`), only to build Hypha itself. Applications are
  plain C++23.

## Install

```bash
git clone https://github.com/sdig-vit-petrik/Hypha.git
cd Hypha
./install.sh            # into /usr/local, or pass a prefix
```

Or build it inside your CMake project:

```cmake
include(FetchContent)
FetchContent_Declare(hypha GIT_REPOSITORY https://github.com/sdig-vit-petrik/Hypha.git GIT_TAG <tag-or-commit>)
set(HYPHA_BUILD_TESTS OFF)
set(HYPHA_BUILD_APPS OFF)
set(HYPHA_BUILD_EXAMPLES OFF)
FetchContent_MakeAvailable(hypha)
```

## A first node

```cmake
cmake_minimum_required(VERSION 3.24)
project(my_robot_node LANGUAGES CXX)
find_package(hypha REQUIRED)
hypha_add_node(doubler src/doubler.cpp)
```

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
  return node.run().ok() ? 0 : 1; // runs until SIGINT or SIGTERM
}
```

A node with its own messages calls `hypha_generate_messages()`. A node that runs at a rate,
or on its inputs, with a TOML file and checked inputs, derives from `hypha::CycleNode`
([cpp-api.md](docs/cpp-api.md), [examples](examples/README.md)).

Look at the running system:

```bash
hypha top
hypha topic list
hypha topic echo /output/data
```

## Work on this repository

```bash
./build.sh [--clean]            # library, SDK, tools, examples, tests
./test.sh [--clean] [rust|cpp]  # clippy and Rust tests, C++ tests, installed-SDK check
./check.sh                      # formatting, links, generated headers, size and dependency rules
./scripts/format.sh             # format Rust, C++ and Python
```

The builder and runtime images are in `docker/Dockerfile` (`./docker/build.sh
builder|runtime`). Run a gate in the builder image with `scripts/in-image.sh ./check.sh`.
Coverage, Miri, fuzzing and sanitizers: [getting-started.md](docs/getting-started.md).

## Documentation

| Document | Covers |
|---|---|
| [Getting started](docs/getting-started.md) | Build, install, CMake, shared-memory trouble |
| [C++ API](docs/cpp-api.md) | Nodes, cycles, ports, input checks, loans, timers, parameters, TF, services |
| [ROS 1 migration](docs/ros1-migration-guide.md) | Porting a ROS 1 C++ node |
| [Execution model](docs/execution-model.md) | `WaitSet`, cycles, callbacks, timers, simulated time |
| [CLI tools](docs/tools.md) | Every `hypha` command |
| [Architecture](docs/architecture.md), [C ABI](docs/interface-specification.md) | Layers, the `libhypha_ffi` contract, the wire frame |
| [Examples](examples/README.md), [apps](apps/README.md) | Runnable nodes, tools, the ROS 1 bridge |
| [Index](docs/README.md) | Everything else: requirements, safety, releases |

## Contributing and license

See [CONTRIBUTING.md](CONTRIBUTING.md). Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)).
