#pragma once

/// @file
/// Single include for an hypha node.
///
///     #include <hypha/hypha.h>
///
///     class Follower : public hypha::CycleNode {
///         hypha::Latest<Pose> pose{*this, "pose"};
///         hypha::Output<Twist> command{*this, "command"};
///         void step(const hypha::CycleTime&) override {
///             if (pose.usable()) command.send(follow(*pose));
///         }
///     };
///
///     int main(int argc, char** argv) {
///         return hypha::runNode<Follower>(argc, argv, "follower");
///     }

#include "hypha/application_node.h"
#include "hypha/callback.h"
#include "hypha/config.h"
#include "hypha/cycle_node.h"
#include "hypha/cycle_timer.h"
#include "hypha/handover.h"
#include "hypha/introspection.h"
#include "hypha/message_size.h"
#include "hypha/message_stamp.h"
#include "hypha/metrics.h"
#include "hypha/node.h"
#include "hypha/publisher.h"
#include "hypha/status.h"
#include "hypha/subscriber.h"
#include "hypha/view.h"
#include "hypha/worker.h"
#include "hypha_base/clock.h"
#include "hypha_base/short_namespace.h"
#include "hypha_base/time.h"

#if !defined(HYPHA_NO_LOGGING)
#    include <hypha_log/hypha_log.h>
#endif
