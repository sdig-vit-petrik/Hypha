# Lanelet2 (vendored)

From https://github.com/fzi-forschungszentrum-informatik/Lanelet2, tag `1.2.3`, commit
`ae39c8d673264afac2339c4f0252df53a7ba82dd` (ae39c8d), the release that ROS 2 Humble and
Jazzy ship, from which Autoware 2.1.0 takes Lanelet2. Licence: `LICENSE` in this directory
(BSD 3-Clause, FZI Forschungszentrum Informatik).

Only the files the port compiles, unchanged, at their upstream paths, each package with its
`package.xml`:

- `lanelet2_core`: `include/` and `src/`, the map and its geometry, on Eigen and
  Boost.Geometry;
- `lanelet2_io`: `include/` and `src/`, reading and writing a map: OSM on pugixml, binary on
  Boost.Serialization, paths on Boost.Filesystem;
- `lanelet2_projection`: `UTM.h`, `LocalCartesian.h` and their `src/` files, on GeographicLib.

They build as shared libraries, as upstream builds them: each map reader registers itself
from a static object of its own file, which a static archive would leave out.
