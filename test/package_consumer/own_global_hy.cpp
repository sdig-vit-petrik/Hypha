// An application with a global hy of its own opts out of the alias; without
// HYPHA_NO_SHORT_NAMESPACE this file does not compile.
#define HYPHA_NO_SHORT_NAMESPACE
#include <hypha/hypha.h>

#include "short_namespace.h"

namespace hy {
int answer()
{
    return 42;
}
}  // namespace hy

bool ownGlobalHyUsable()
{
    return hy::answer() == 42 && hypha::now().nanos() >= 0;
}
