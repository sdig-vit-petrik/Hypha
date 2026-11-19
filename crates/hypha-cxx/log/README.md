# hypha_log

`hypha_log` is the logging macro header of the C++ SDK. The SDK installs it
with `hypha`. It is not a separate CMake package.

The header is a thin shim. The logging engine is the Rust crate `hypha_log`.
It is in `libhypha_ffi.so`, and the shim calls it through the `hypha_log_*`
C functions. Logging is synchronous. The engine creates no thread, and a line
is written on the thread that logs.

## Use

```cpp
#include <hypha_log/hypha_log.h>

HYPHA_LOG_MODULE_INIT("localisation")

void start(double rate)
{
  HYPHA_LOG_INFO("started at {} Hz", rate);
  HYPHA_LOG_WARN_THROTTLE(1.0, "input is unavailable");
}
```

`HYPHA_LOG_MODULE_INIT(name)` has one argument. It selects the module for the
translation unit. Without it, the translation unit uses the module `default`.
Put it before the first log macro of the translation unit. A macro
used before it writes under the module `default`.

## Macros

- `HYPHA_LOG_<LEVEL>(fmt, ...)`: `<LEVEL>` is `TRACE`, `DEBUG`, `INFO`,
  `WARN`, `ERROR` or `FATAL`. `FATAL` does not stop the process.
- `HYPHA_LOG_<LEVEL>_THROTTLE(period_s, fmt, ...)`: the first call writes.
  Later calls write at most once for each period.
- `HYPHA_LOG_<LEVEL>_WHEN(state, initial, fmt, ...)`: writes when `state`
  changes from false to true.
- `HYPHA_LOG_<LEVEL>_WHEN_THROTTLE(state, initial, period_s, fmt, ...)`:
  writes on the change, then at most once for each period while `state` is true.

`fmt` is a `std::format_string`. The compiler checks it. The macro evaluates
the arguments only when the line has a target. Throttling uses host monotonic
time.

## Configuration

The `[log]` table of the node parameter file is the only configuration. There
is no environment variable and no second file. The logging section of
`docs/cpp-api.md` gives the keys; the validation rules and file rotation are in
the logging engine section of `docs/interface-specification.md`.

## Install

The `hypha-cxx` package installs `hypha_log/hypha_log.h` and
`hypha_log/detail/call_site.h`. It installs no other logging header.
