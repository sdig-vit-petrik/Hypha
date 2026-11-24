// Every name of hypha here is reached through the alias hy.
#include "short_namespace.h"

#include <hypha/hypha.h>

#include <std_msgs/Float64.h>

bool shortNamespaceUsable()
{
    const hy::Time     current = hy::now();
    const hy::Duration step = hy::Duration::fromMillis(5);
    std_msgs::Float64  value;
    value.data = 2.0;
    return current.nanos() >= 0 && step.nanos() == 5'000'000 && value.data == 2.0;
}
