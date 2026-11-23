# autoware_lanelet2_extension (vendored)

From https://github.com/autowarefoundation/autoware_lanelet2_extension, tag `1.3.0`, commit
`b1df9ed04d361562b08b1a0ac33451c2031b8f8c` (b1df9ed), the version that Autoware 2.1.0 pins.
Licence: `LICENSE` in this directory (Apache 2.0).

Only the projectors `autoware_geography_utils` uses, unchanged, at their upstream paths:
`autoware_lanelet2_extension/include/autoware_lanelet2_extension/projection/`
`mgrs_projector.hpp` and `transverse_mercator_projector.hpp`, their `lib/` sources, and the
package's `package.xml`. They are Lanelet2 projectors on GeographicLib (`../lanelet2/`).
