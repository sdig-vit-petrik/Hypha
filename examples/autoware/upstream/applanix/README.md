# applanix applanix_driver (vendored)

From https://github.com/autowarefoundation/applanix, branch `main`, commit
`6689c1ba2796b2c6cc77a6e9c5dd1c1507d8f862` (6689c1b), the commit `../../msg/applanix_msgs/`
comes from, imported with it. Licence: `LICENSE` in this directory (Apache 2.0; the
sources carry LeoDrive.ai's copyright, `package.xml` says `none`).

Only what `../../tools/applanix_lvx_client_replay.cpp` needs to run the driver's own
conversions on recorded GSOF 49 and 50, unchanged, at their upstream paths under
`applanix_driver/`, with the package's `package.xml`:

- `include/applanix_driver/gsof/` `gsof.h` and `message.h`, the GSOF records;
- `include/applanix_driver/math.h` and `include/applanix_driver/utils/byteswap.h`, which
  they use;
- `include/applanix_driver_ros/conversions.h` and `src/applanix_driver_ros/conversions.cpp`,
  the GSOF to NavSatFix, orientation and twist conversions;
- `config/lvx_client_params.param.yaml`, the driver's parameters, kept as the reference.
