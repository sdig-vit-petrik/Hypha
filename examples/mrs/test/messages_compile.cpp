/// Every generated mrs_msgs header, the example's std_srvs and mrs_bridge compile in one unit.
#include "all_mrs_msgs.h"

#include <mrs_bridge/Call.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

int main()
{
    const mrs_msgs::UavState state{};
    return state.header.seq == 0 ? 0 : 1;
}
