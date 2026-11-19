#pragma once

/// @file
/// The part of roscpp that the ported MRS code touches, over hypha. Never installed.

#include <ros/console.h>
#include <ros/time.h>
#include <ros/timer.h>

#include <string>

namespace mrs {
/// The hypha node name, set by mrs::PluginHost.
const std::string& nodeName();
void               setNodeName(std::string name);
}  // namespace mrs

namespace ros {

class NodeHandle;

namespace this_node {
/// The hypha node name with a leading '/'.
std::string getName();
}  // namespace this_node

/// Ends the run: hypha::requestStop().
void shutdown();
bool ok();

/// Transport hints have no meaning on hypha.
struct TransportHints
{
    TransportHints& tcpNoDelay(bool = true)
    {
        return *this;
    }
};

}  // namespace ros
