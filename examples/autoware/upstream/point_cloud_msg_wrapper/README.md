# point_cloud_msg_wrapper (vendored)

From https://gitlab.com/ApexAI/point_cloud_msg_wrapper, branch `master`, commit
`0a62affd736fc39744467ff3a68d7e1f6c3d1bb5` (0a62aff), the package `autoware_point_types`
includes. Licence: `LICENSE` in this directory (Apache 2.0, Apex.AI).

Only `include/point_cloud_msg_wrapper/`, unchanged. `../patches/point_cloud_msg_wrapper.patch`
adds one include that upstream lacks (`<algorithm>`, which ROS's headers bring in). The
header relies on what ROS generated C++ provides: `_<field>_type` and `set__<field>()`
members, which hypha's generator emits, and the `PointCloud2_<Allocator>` templates, which
the shim's message headers declare.
