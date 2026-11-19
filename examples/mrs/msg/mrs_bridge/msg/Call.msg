# One request or one result of a service carried over topics (mrs::Serve, mrs::Request).
# caller: node name of the client; id: the client's sequence number;
# payload: the serialized service Request or Response.
string caller
uint64 id
uint8[] payload
