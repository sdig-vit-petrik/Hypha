# How far one node's replay has run, published on /clock_progress once per /clock sample.
# clock_nanos is the simulated time of the newest /clock sample the node read.
# clock_sent_nanos is the transport send time of that sample, which the player
# compares with its own monotonic clock to know the node has read it.
string node
uint64 clock_nanos
uint64 clock_sent_nanos
