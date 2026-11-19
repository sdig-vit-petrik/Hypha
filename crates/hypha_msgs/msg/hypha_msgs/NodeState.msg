# The input health of one hypha node, published on ~/state every second.

uint8 INPUTS_USABLE=0
uint8 WAITING=1
uint8 DEGRADED=2

string node
uint8 state
hypha_msgs/RequiredInput[] inputs
uint64 callbacks_invoked
uint64 callback_failures
uint64 slow_callbacks
hypha_msgs/CycleState[] cycles
uint64 log_lines_dropped
uint64 log_write_failures
bool sim_time
