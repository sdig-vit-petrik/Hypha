# The execution counters of one cycle of a hypha node, in hypha_msgs/NodeState.
#
# The step durations are host monotonic time. The distribution is a histogram of 16 buckets per
# power of two: a quantile is at most 1/16 above the true value and never below it.

string name
uint64 steps
uint64 missed_periods
uint64 last_step_nanos
# The longest step since the cycle was committed.
uint64 max_step_nanos
uint64 overruns
uint64 output_drops
# The median, the 90th and the 99th percentile of every step since the cycle was committed.
uint64 step_p50_nanos
uint64 step_p90_nanos
uint64 step_p99_nanos
# The same for the steps of the last 10 to 20 seconds, and their longest.
uint64 recent_step_p50_nanos
uint64 recent_step_p90_nanos
uint64 recent_step_p99_nanos
uint64 recent_step_max_nanos
