# autoware_map_msgs

`MapProjectorInfo` of https://github.com/autowarefoundation/autoware_msgs, tag `1.13.0`
(commit `bb8e7bf5d97168663e0e7b357929e4fcdbd3a967`), the version Autoware 2.1.0 pins: the
map's projection, which `autoware_gnss_poser` reads. Licence: `LICENSE` (Apache 2.0), with
its `NOTICE`.

`MapProjectorInfo.msg` is rewritten in ROS 1 syntax until hypha reads ROS 2 definitions
(#45): a ROS 1 string constant keeps its quotes, so `string LOCAL = "Local"` stands as
`string LOCAL = Local`, the value ROS 2 gives it. Otherwise unchanged.
