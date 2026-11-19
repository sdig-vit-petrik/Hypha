#pragma once

/// @file
/// The one assertion the MRS checks use: print the failed condition, keep going, fail at exit.

#include <cstdio>

namespace mrs_test {
inline int& failures()
{
    static int count = 0;
    return count;
}
}  // namespace mrs_test

#define MRS_CHECK(cond)                                                                            \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            std::fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #cond);                        \
            ++mrs_test::failures();                                                                \
        }                                                                                          \
    } while (false)

inline int mrsCheckResult()
{
    return mrs_test::failures() == 0 ? 0 : 1;
}
