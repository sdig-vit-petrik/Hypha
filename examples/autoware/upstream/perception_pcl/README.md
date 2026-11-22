# perception_pcl pcl_conversions (vendored)

From https://github.com/ros-perception/perception_pcl, branch `jazzy`, commit
`c5e7e72d20cbbc164e9d940f75ae2a6ae5bbf41a` (c5e7e72): only
`pcl_conversions/include/pcl_conversions/pcl_conversions.h`, the header-only conversions
between sensor_msgs and PCL, and its `package.xml`. Licence: BSD-3-Clause, in the header
and in `package.xml`.

`../patches/pcl_conversions.patch` drops its two `message_filters` includes, which it does
not use.
