#pragma once

/// @file
/// ros::package::getPath for the vendored upstream repositories only.

#include <string>

namespace ros::package {

/// The vendored directory of @p package under examples/mrs/upstream, or "" (with an
/// error log) for a package that is not vendored.
std::string getPath(const std::string& package);

}  // namespace ros::package
