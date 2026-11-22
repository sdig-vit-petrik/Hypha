# autoware_msgs (vendored)

From https://github.com/autowarefoundation/autoware_msgs, tag `1.13.0`, commit
`bb8e7bf5d97168663e0e7b357929e4fcdbd3a967` (bb8e7bf), the version that Autoware 2.1.0
pins. Licence: `LICENSE` in this directory (Apache 2.0), with its `NOTICE`, which a copy must
carry.

Only the messages the ported nodes publish, unchanged, each package with its
`package.xml`:

- `autoware_perception_msgs`: `DetectedObjects`, `DetectedObject`,
  `DetectedObjectKinematics`, `ObjectClassification` and `Shape`;
- `autoware_sensing_msgs`: `ConcatenatedPointCloudInfo` and `SourcePointCloudInfo`.

Their ROS 2 definitions are valid ROS 1 syntax, so hypha generates them as they are
(`hypha_generate_messages`, `../../CMakeLists.txt`).
