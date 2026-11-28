# One finished call of a service, as its client saw it: the call log.
# Each client publishes it on <service>/calls when the call ends, while someone
# subscribes. hypha record records it; hypha play never republishes it.

uint8 ANSWERED=0
uint8 REFUSED=1
uint8 NO_SERVER=2
uint8 NOT_SENT=3
uint8 SERVER_LOST=4
uint8 TIMEOUT=5

# The resolved service name, its .srv type and its MD5.
string service
string type
string md5
# The client's node, and its port: the ports-table entry, or the service it named.
string client_node
string client_port
# The request's sequence of this client; 0 for a call never sent.
uint64 sequence
uint8 outcome
# What became of the call, as describe() gives it: "refused: not armed".
string reason
# The client's framework time at the call and at its outcome.
time sent
time done
# The encoded request and, for an answered call, the encoded response. Each is cut
# to its first 16384 bytes, whatever the service policy allows.
uint8[] request
uint8[] response
# The lengths of the request and the response before the cut, and whether either was
# cut (request_len or response_len above the bytes it carries).
uint64 request_len
uint64 response_len
bool truncated
