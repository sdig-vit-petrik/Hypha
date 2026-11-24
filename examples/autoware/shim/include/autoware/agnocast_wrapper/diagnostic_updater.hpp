#pragma once

/// @file
/// autoware_agnocast_wrapper's diagnostic updater without Agnocast: upstream forwards to
/// ::diagnostic_updater::Updater on the node, which is the shim's.

#include <autoware/agnocast_wrapper/node.hpp>
#include <diagnostic_updater/diagnostic_updater.hpp>

namespace autoware::agnocast_wrapper::diagnostic_updater {
using Updater = ::diagnostic_updater::Updater;
}  // namespace autoware::agnocast_wrapper::diagnostic_updater
