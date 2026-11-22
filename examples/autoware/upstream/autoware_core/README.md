# autoware_core (vendored)

From https://github.com/autowarefoundation/autoware_core, tag `1.10.0`, commit
`384bcd47e6d2fd21e5954a915153aa5303b34a53` (384bcd4), the version that Autoware 2.1.0
pins in `repositories/autoware.repos`.
Licence: `LICENSE` in this directory (Apache 2.0, the Autoware Foundation and TIER IV).

Only the packages the hypha port compiles are copied, at their upstream paths:

- `sensing/autoware_crop_box_filter`: `src/`, `test/`, `config/`, `package.xml`, `README.md`.

The files are unchanged. The edits the port needs are in `../patches/`, one file per
package; the build applies them to a copy in the build directory.
