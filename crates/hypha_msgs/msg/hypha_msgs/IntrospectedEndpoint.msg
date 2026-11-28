# One topic endpoint of a node, as hypha::introspect() reports it.
# hypha formats nothing: a tool or an application decides what to show.

uint8 OUTPUT_NONE=0
uint8 OUTPUT_FAIL_RUN=1
uint8 OUTPUT_DROP_POOL_EXHAUSTED=2

# Names this row while its endpoint lives. An input binding names its
# subscriber by it. Never 0.
uint64 id
string topic
# The message type, such as std_msgs/Float64. Empty for an untyped endpoint.
string data_type
# The MD5 of the message definition: with the topic it names the endpoint's
# stream, so two versions of one type on one topic are two endpoints. Empty for
# an untyped endpoint.
string md5
# The receive queue depth of a subscriber; 0 otherwise.
uint32 queue_depth
# A publisher that latches its last message.
bool latched
# The cycle whose output owns this publisher; empty otherwise.
string output_cycle
# What that output does when the publisher refuses a message; OUTPUT_NONE
# when no cycle output owns the publisher.
uint8 output_failure
