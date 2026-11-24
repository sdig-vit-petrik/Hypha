#pragma once

/// @file
/// `hy` is a second name of the namespace `hypha`: `hy::Node` is `hypha::Node`. It is an
/// alias, so types, argument-dependent lookup, specializations and messages keep the
/// name `hypha`. Every public entry header includes this one.
///
/// The alias takes the global name `hy`. An application with a global `hy` of its own
/// defines `HYPHA_NO_SHORT_NAMESPACE` before it includes a hypha header.

#if !defined(HYPHA_NO_SHORT_NAMESPACE)
namespace hypha {
}
namespace hy = hypha;
#endif
