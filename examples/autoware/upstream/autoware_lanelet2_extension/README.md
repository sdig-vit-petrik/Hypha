# autoware_lanelet2_extension (vendored)

From https://github.com/autowarefoundation/autoware_lanelet2_extension, tag `1.3.0`, commit
`b1df9ed04d361562b08b1a0ac33451c2031b8f8c` (b1df9ed), the version that Autoware 2.1.0 pins.
Licence: `LICENSE` in this directory (Apache 2.0).

Only these files, unchanged, at their upstream paths under `autoware_lanelet2_extension/`,
with the package's `package.xml`:

- `include/autoware_lanelet2_extension/projection/` `mgrs_projector.hpp` and
  `transverse_mercator_projector.hpp`, and their `lib/` sources: Lanelet2 projectors on
  GeographicLib (`../lanelet2/`), which `autoware_geography_utils` uses;
- `include/autoware_lanelet2_extension/io/autoware_osm_parser.hpp`, its `lib/` source and
  `version.hpp`: the OSM parser of Autoware's map format, which
  `autoware_map_projection_loader` reads a map with.
