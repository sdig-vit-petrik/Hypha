# One cycle input binding, including a binding with no live source.

uint8 MODE_LATEST=0
uint8 MODE_QUEUED=1

string cycle
# The declaration-order index of the input in its cycle.
uint32 index
# The topic, or empty when the input is unwired.
string topic
# The id of the subscriber row this binding reads; 0 when unwired. The
# bindings of one shared source name the same subscriber.
uint64 subscriber_id
# Node health judges this binding.
bool required
uint8 mode
# MODE_QUEUED only: samples examined per iteration. Pending samples beyond that
# share drop the oldest.
uint32 max_samples_per_iteration
# Staleness and stagnation are supervised. The fields below are 0 otherwise.
bool supervised
# The silence limit: the most a sample may age after it was sent before the input
# is stale; 0 means no limit.
uint64 silence_nanos
# The longest time without a changed sample; 0 means the check is off.
uint64 stagnation_nanos
# The application currently ignores the faults of this binding.
bool ignored
