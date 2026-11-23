# One request or response of an rclcpp service on hypha (shim/include/rclcpp/rclcpp.hpp,
# "Services"). The client publishes it on <service>/request; the server answers on
# <service>/response with the same caller and id. payload is the service's Request or
# Response, serialized.
string caller
uint64 id
# A response only: false when the server could not read the request; payload is empty.
bool handled
uint8[] payload
