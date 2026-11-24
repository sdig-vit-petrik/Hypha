#pragma once

/// @file
/// tl::expected, which Autoware uses, is C++23 std::expected under its old name.

#include <expected>

namespace tl {
using std::expected;
using std::unexpected;
}  // namespace tl
