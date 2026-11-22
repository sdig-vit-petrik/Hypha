#pragma once

/// @file
/// The part of {fmt} the ported code uses, fmt::format with a literal format string:
/// std::format, which {fmt} became in C++20.

#include <format>

namespace fmt {
using std::format;
}  // namespace fmt
