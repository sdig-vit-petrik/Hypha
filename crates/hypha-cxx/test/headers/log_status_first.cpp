#include "hypha/status.h"
#include "hypha_log/hypha_log.h"

#include <string_view>
#include <type_traits>

static_assert(
    std::is_same_v<decltype(hypha::log::detail::registerModule(std::string_view{})), HyphaLogModule>
);
