# Lanelet2 (vendored)

From https://github.com/fzi-forschungszentrum-informatik/Lanelet2, tag `1.2.3`, commit
`ae39c8d673264afac2339c4f0252df53a7ba82dd` (ae39c8d), the release that ROS 2 Humble and
Jazzy ship, from which Autoware 2.1.0 takes Lanelet2. Licence: `LICENSE` in this directory
(BSD 3-Clause, FZI Forschungszentrum Informatik).

Only the files the projectors of `autoware_geography_utils` compile, unchanged, at their
upstream paths, each package with its `package.xml`:

- `lanelet2_projection`: `UTM.h`, `LocalCartesian.h` and their `src/` files, on GeographicLib;
- `lanelet2_io`: `Projection.h` (the `Projector` base and `Origin`) and `Exceptions.h`;
- `lanelet2_core`: the headers those include: `primitives/` `Point.h`, `GPSPoint.h`,
  `Primitive.h`, `Traits.h`; `Attribute.h`, `Exceptions.h`, `Forward.h`; `utility/`
  `Utilities.h`, `Optional.h`, `TransformIterator.h`, `HybridMap.h`. They are header-only
  on Eigen and Boost.

The map, its geometry and its reading and writing are not vendored.
