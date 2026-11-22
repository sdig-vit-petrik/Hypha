# autoware_ground_filter: unqualified `abs()` on `float` truncates heights to `int`

> The report to file at https://github.com/autowarefoundation/autoware_core/issues. The hypha port carries the fix in [`autoware_ground_filter.patch`](../examples/autoware/upstream/patches/autoware_ground_filter.patch) until upstream has it ([examples/autoware](../examples/autoware/README.md), #47).

## Summary

`perception/autoware_ground_filter/src/ground_filter.cpp` compares float heights through an unqualified `abs()`. Name lookup can then pick C's `int abs(int)` from `<stdlib.h>` instead of the `float` overload of `<cmath>`, and that silently truncates every value below 1 m to `0`. Which overload is chosen depends only on what other headers happen to include. With PCL 1.15 and libstdc++ (Ubuntu 26.04, GCC 15.2) all eight calls become `int abs(int)`, and the grid-mode classification changes.

The grid-mode code in `grid.hpp` already calls `std::abs`.

## Where

`ground_filter.cpp`, unchanged from 1.10.0 (`384bcd4`) to `main` (`15408eb`, 2026-11-20):

| Line | Call |
|---|---|
| 198 | `abs(height) < global_slope_threshold && abs(height) < param_.non_ground_height_threshold` |
| 253 | `abs(delta_z) < global_slope_max_ratio_ * delta_radius` |
| 271 | `abs(delta_gnd_z) <= gnd_z_threshold` |
| 310 | `abs(delta_avg_z) < global_slope_threshold` |
| 317 | `abs(delta_avg_z) < param_.non_ground_height_threshold` |
| 324 | `abs(delta_max_z) < param_.non_ground_height_threshold` |
| 371 | `abs(delta_z) < global_slope_threshold` |

The file includes neither `<cmath>` nor `<cstdlib>` itself.

## Why it depends on the toolchain

The `float` and `double` overloads of `abs` are declared in namespace `std` by `<cmath>`. A global `::abs(float)` exists only if some header brings it in, for example libstdc++'s `<math.h>` wrapper (`using std::abs;`) or libc++'s `<math.h>`. glibc's `<stdlib.h>`, which almost every header pulls in, declares only `int abs(int)`.

- **libc++ (macOS, clang):** the `float` overload is global; the code works.
- **libstdc++ with PCL 1.14 (Ubuntu 24.04):** a PCL header apparently brings in `<math.h>`; the code works.
- **libstdc++ with PCL 1.15 (Ubuntu 26.04, GCC 15.2.0, arm64 checked):** only `int abs(int)` is visible. Each `float` argument is converted to `int`, so any `|x| < 1.0` becomes `0`.

The thresholds involved are well under a metre (`non_ground_height_threshold` 0.2 m in the default config), so the comparisons are decided by the truncated value: a 0.3 m height compares as 0.

## How to see it

Compile `ground_filter.cpp` with `-Wfloat-conversion` against PCL 1.15 on Ubuntu 26.04:

```
ground_filter.cpp:198:13: warning: conversion from 'float' to 'int' may change value [-Wfloat-conversion]
  198 |         abs(height) < global_slope_threshold && abs(height) < param_.non_ground_height_threshold) {
      |             ^~~~~~
ground_filter.cpp:253:13: warning: conversion from 'float' to 'int' may change value [-Wfloat-conversion]
...
```

All eight calls are reported. With the same build, a grid-mode test scene (flat ground rings 3 m to 15 m around the vehicle, and a 20-point pole from 0.5 m to 1.5 m high, 8 m ahead) keeps 10 of the 20 pole points. With `std::abs`, or on macOS with libc++, it keeps all 20 and no ground point. The upstream unit tests (`test_ground_filter.cpp`) pass either way: they check that filtering succeeds and returns points, not which points.

## Fix

Call `std::abs` and include `<cmath>`:

```diff
+#include <cmath>
 ...
-        abs(height) < global_slope_threshold && abs(height) < param_.non_ground_height_threshold) {
+        std::abs(height) < global_slope_threshold &&
+        std::abs(height) < param_.non_ground_height_threshold) {
```

and the same at lines 253, 271, 310, 317, 324 and 371.

To catch the next one: `-Wfloat-conversion`, or clang-tidy's `bugprone-narrowing-conversions` (or `cppcoreguidelines-narrowing-conversions`), reports these calls on any toolchain where the `int` overload wins. A unit test that asserts which points of a known scene are kept, not only that some are, would also have caught it.

## Environment where it was found

- **OS and toolchain:** Ubuntu 26.04 (arm64), GCC 15.2.0, libstdc++, PCL 1.15 (`libpcl-dev`).
- **Source:** `autoware_core` 1.10.0, the version Autoware 2.1.0 pins.
- **Found by:** a port of the node to another middleware, which builds the upstream sources unchanged. The port's node test caught it, and `-Wfloat-conversion` confirmed it. The same sources pass on macOS (Apple clang 21, libc++, Homebrew PCL 1.15).
