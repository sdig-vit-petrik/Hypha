# autoware_map_msgs

The messages and services of `autoware_map_msgs` in
https://github.com/autowarefoundation/autoware_msgs, tag `1.13.0` (commit
`bb8e7bf5d97168663e0e7b357929e4fcdbd3a967`), the version Autoware 2.1.0 pins, that the
ported code names: `MapProjectorInfo`, the map's projection, which
`autoware_map_projection_loader` publishes and `autoware_gnss_poser` reads; and the map
interface of `autoware_component_interface_specs`' `map.hpp`: `LaneletMapBin`, the services
`GetDifferentialPointCloudMap` and `GetPartialPointCloudMap`, and `AreaInfo`,
`PointCloudMapCellWithID` and `PointCloudMapCellMetaData` that they hold. Licence:
`LICENSE` (Apache 2.0), with its `NOTICE`.

All but `MapProjectorInfo.msg` are unchanged: their ROS 2 definitions are valid ROS 1
syntax.

`MapProjectorInfo.msg` is rewritten in ROS 1 syntax until hypha reads ROS 2 definitions
(#45): a ROS 1 string constant keeps its quotes, so `string LOCAL = "Local"` stands as
`string LOCAL = Local`, the value ROS 2 gives it. Otherwise unchanged.
