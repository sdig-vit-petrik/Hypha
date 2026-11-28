# Standard services of ROS 1 (vendored)

The `.srv` files of the standard packages the SDK ships messages for, unchanged, so that
the MD5 of each service is the one of ROS 1 (Issue #315). They sit in `../msg/<package>/`
beside the messages, which is where the SDK installs and reads them; the manifests here
record where each came from and its SHA-256.

| Directory | Source | Services |
| --------- | ------ | -------- |
| `common_msgs/` | https://github.com/ros/common_msgs, tag `1.13.2` (the last ROS 1 Noetic release), commit `1230f39a7068d1d73d1039eb0eb970c922b6bcf7` | `nav_msgs/GetMap`, `GetPlan`, `LoadMap`, `SetMap`; `sensor_msgs/SetCameraInfo`; `diagnostic_msgs/AddDiagnostics`, `SelfTest` |
| `geometry2/` | https://github.com/ros/geometry2, tag `0.7.10` (Noetic), commit `40ce3df158c80cc4ac5edc5b1c22fe833d0cbc4c` | `tf2_msgs/FrameGraph` |

`std_srvs` (`Empty`, `Trigger`, `SetBool`) predates this record and is not listed.
`geometry_msgs`, `std_msgs` and `rosgraph_msgs` have no services in ROS 1.

Licence: BSD 3-Clause, the licence of the ROS 1 packages (`package.xml` of each); the
files carry no header of their own, so the licence is stated in `NOTICE`.

`python3 -I examples/autoware/tools/port.py verify` (run by `./check.sh`) checks the
SHA-256 of each file. A manifest names a `root` (`../../msg`): only the files it lists are
checked there. To fetch them again: `python3 -I examples/autoware/tools/port.py vendor
hypha_msgs/common_msgs` (or `hypha_msgs/geometry2`).

The MD5 of each service is pinned in `crates/hypha_msgdef/tests/fixtures/golden_md5.txt`
(`golden_md5.rs`), and was checked against `genmsg` of ros/genmsg, which computes it in
ROS 1.
