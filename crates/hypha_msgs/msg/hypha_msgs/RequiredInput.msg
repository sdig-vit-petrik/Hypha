# One input that an input set requires, as node health judged it.

uint8 USABLE=0
uint8 NEVER_ARRIVED=1
uint8 STALE=2

string topic
# The message type and the MD5 of the input: one topic carries one stream per
# schema, and each is its own row.
string data_type
string md5
uint8 verdict
uint64 age_nanos
bool stagnant
bool ignored
uint64 dropped
uint64 rejected
uint64 undecodable
# The other schemas of the topic that a live publisher sends, as the node's
# housekeeping last found them, at most a second old while the node runs: a
# publisher of one of them sends nothing this input reads. Not a fault by itself:
# a migration has two. other_schemas_checked is false before the first check,
# and other_schemas is then empty.
bool other_schemas_checked
hypha_msgs/TopicSchema[] other_schemas
