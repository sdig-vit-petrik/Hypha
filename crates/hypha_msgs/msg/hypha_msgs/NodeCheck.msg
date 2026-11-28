# One named check that the application of a node reports about itself (a device is
# connected, a fix is held, a calibration is loaded), in hypha_msgs/NodeState. The
# levels are those of diagnostic_msgs/DiagnosticStatus.

# Nothing wrong.
uint8 OK=0
# Shown by the node's state, no change in whether the node is ready.
uint8 WARN=1
# The node cannot do its work: its state is DEGRADED, as for a required input that is not ok.
uint8 ERROR=2
# The check cannot say (its data is old): counted as ERROR.
uint8 STALE=3

string name
uint8 level
string message
