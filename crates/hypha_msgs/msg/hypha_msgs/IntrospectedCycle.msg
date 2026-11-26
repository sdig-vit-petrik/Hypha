# One cycle of a node. period_nanos is meaningful only when has_period is true.
# triggers are the topics a triggered cycle runs on, in port order, none for a rate cycle.
string name
bool has_period
uint64 period_nanos
uint32 inputs
uint32 outputs
string[] triggers
