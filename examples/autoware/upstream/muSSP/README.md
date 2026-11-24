# muSSP (vendored)

From https://github.com/tier4/muSSP, commit `c79e98fd5e658f4f90c06d93472faa977bc873b9`
(c79e98f), the version Autoware 2.1.0 pins in `repositories/autoware.repos`: the
min-cost-flow solver of `autoware_multi_object_tracker`'s data association. Licence:
`LICENSE` in this directory (Apache 2.0).

Only the library, unchanged: `muSSP/` `Graph`, `Node`, `Sink` and `mussp` sources and
headers, and `package.xml`. The repository's FollowMe and SSP programs and its data are
not copied.
