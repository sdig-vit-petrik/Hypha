#pragma once

/// @file
/// glibc's <byteswap.h> on any compiler: bswap_16, bswap_32 and bswap_64 on the compiler's
/// builtins, as glibc defines them. The Applanix driver includes it; macOS has no such
/// header.

#include <cstdint>

inline uint16_t bswap_16(uint16_t value)
{
    return __builtin_bswap16(value);
}

inline uint32_t bswap_32(uint32_t value)
{
    return __builtin_bswap32(value);
}

inline uint64_t bswap_64(uint64_t value)
{
    return __builtin_bswap64(value);
}
