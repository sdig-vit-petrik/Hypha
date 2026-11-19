# What the engine knows about one node, taken at one instant: its identity, the
# configuration keys it read, and every endpoint and cycle it holds. Rows are
# in registration order. hypha shows none of it; a tool or library renders it.
string node
string domain
int32 pid
# The wall-clock creation time, in ISO 8601 UTC.
string created
# The version of the hypha framework.
string hypha
hypha_msgs/IntrospectedConfig[] configuration
hypha_msgs/IntrospectedEndpoint[] subscribers
hypha_msgs/IntrospectedEndpoint[] publishers
hypha_msgs/IntrospectedCycle[] cycles
hypha_msgs/IntrospectedInput[] input_bindings
