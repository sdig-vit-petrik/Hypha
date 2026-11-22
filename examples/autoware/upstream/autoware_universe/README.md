# autoware_universe (vendored)

From https://github.com/autowarefoundation/autoware_universe, tag `0.53.2`, commit
`62a5739d4d5a34f27f6aa15702c8ff59fa4d862c` (62a5739), the version that Autoware 2.1.0
pins in `repositories/autoware.repos`.
Licence: `LICENSE` in this directory (Apache 2.0), with its `NOTICE`, which a copy must
carry.

Only the files the hypha port compiles are copied, at their upstream paths:

- `sensing/autoware_pointcloud_preprocessor`: the concatenate-and-time-sync node, that
  is `include/autoware/pointcloud_preprocessor/concatenate_data/` and
  `src/concatenate_data/` without the older `concatenate_pointclouds` node, the
  `utility/memory` layout checks and `diagnostics/format_utils.hpp` they use, three
  unit tests (`test_concatenation_info.cpp`, `test_concatenation_diagnostics.cpp`,
  `test_concatenate_node_unit.cpp`), `config/concatenate_and_time_sync_node.param.yaml`,
  `docs/concatenate-data.md` and `package.xml`.

The files are unchanged. The edits the port needs are in
`../patches/autoware_pointcloud_preprocessor.patch`; the build applies it to a copy in
the build directory.
