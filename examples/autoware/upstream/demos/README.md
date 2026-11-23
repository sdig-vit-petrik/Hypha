# demos logging_demo (vendored)

From https://github.com/ros2/demos, branch `jazzy`, commit
`37e25e0dece32f972b3cc8677b6aab3e891a9365` (37e25e0): only
`logging_demo/srv/ConfigLogger.srv`, the service that `autoware_utils_logging`'s
`LoggerLevelConfigure` offers on `~/config_logger`.
Licence: `LICENSE` in this directory (Apache 2.0).

The file is unchanged. hypha sets log levels from the parameter file's `[log]` table only,
so the shim's `LoggerLevelConfigure` refuses every request
(`../../shim/include/autoware_utils_logging/logger_level_configure.hpp`).
