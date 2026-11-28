#pragma once

// Fixtures the DiagnosticUpdater tests share: a sink in place of a port, a cycle node that
// sends on its Output, and a time in seconds.

#include "hypha/cycle_node.h"
#include "hypha/diagnostic_updater.h"

#include <diagnostic_msgs/DiagnosticArray.h>

#include <vector>

namespace diagnostic_test {

using diagnostic_msgs::DiagnosticArray;
using diagnostic_msgs::DiagnosticStatus;
using hypha::DiagnosticStatusWrapper;
using hypha::DiagnosticUpdater;
using hypha::Time;

inline Time at(double seconds)
{
    return Time::fromSeconds(seconds);
}

/// What the updater sends to, in place of a port.
struct Collect
{
    std::vector<DiagnosticArray> sent;

    void send(const DiagnosticArray& array)
    {
        this->sent.push_back(array);
    }
};

/// A node as a port writes it: the updater sends on the Output of the cycle.
class Driver final : public hypha::CycleNode
{
  public:
    Output<DiagnosticArray> diagnostics{*this, "diagnostics"};
    DiagnosticUpdater       updater{this->diagnostics, "driver"};
    bool                    connected{true};

    Driver()
    {
        this->updater.setHardwareID("hesai");
        this->updater.add("Connection", this, &Driver::diagnoseConnection);
        this->updater.add("Cycle", [](DiagnosticStatusWrapper& status) {
            status.summary(DiagnosticStatus::OK, "stepping");
        });
    }

    void step(const hypha::CycleTime& time) override
    {
        this->updater.update(time.now);
    }

  private:
    void diagnoseConnection(DiagnosticStatusWrapper& status)
    {
        if (this->connected) {
            status.summary(DiagnosticStatus::OK, "connected");
        } else {
            status.summary(DiagnosticStatus::ERROR, "device absent");
        }
    }
};

}  // namespace diagnostic_test
