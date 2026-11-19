/// The scan cycle as a process: one optional argument, the parameter file. runNode loads
/// it, constructs the node, starts the cycle, publishes `~/state` and `~/introspection`,
/// and maps every way out to its exit code (0, 2, 70, 75, 78).
#include <hypha/hypha.h>

// Before the node's header: the log lines of its step belong to this module.
HYPHA_LOG_MODULE_INIT("scan_cycle")

#include "scan_cycle/scan_cycle.h"

int main(int argc, char** argv)
{
    return hypha::runNode<scan_cycle::ScanCycle>(argc, argv, "scan_cycle");
}
