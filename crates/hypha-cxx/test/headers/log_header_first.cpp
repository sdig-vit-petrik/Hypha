// The log header before any other hypha header.
// clang-format off
#include "hypha_log/hypha_log.h"
#include "hypha/status.h"
// clang-format on

#include <string_view>
#include <type_traits>

static_assert(
    std::is_same_v<decltype(hypha::log::detail::registerModule(std::string_view{})), HyphaLogModule>
);
