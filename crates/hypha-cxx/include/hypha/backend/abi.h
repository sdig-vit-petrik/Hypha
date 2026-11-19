#pragma once

/// @file
/// Exact client/library ABI revision check used before SDK session creation.

#include "hypha/hypha_ffi.h"

#include <cstdint>
#include <string>

namespace hypha::backend {

inline bool abiRevisionMatches(std::uint32_t expected, std::uint32_t actual)
{
    return actual == expected;
}

/// @brief Refuse startup unless the loaded library has this SDK's exact ABI.
inline bool verifyAbi(std::string* diagnostic = nullptr)
{
    const std::uint32_t actual = hypha_abi_revision();
    if (abiRevisionMatches(HYPHA_ABI_REVISION, actual)) {
        return true;
    }
    if (diagnostic != nullptr) {
        *diagnostic = "hypha ABI revision mismatch: SDK expects "
                    + std::to_string(HYPHA_ABI_REVISION) + ", library reports "
                    + std::to_string(actual);
    }
    return false;
}

}  // namespace hypha::backend
