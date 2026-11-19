# One configuration value of a node, as text.
uint8 SOURCE_PARAMETER=0
uint8 SOURCE_APPLICATION=1
uint8 SOURCE_COMMAND_LINE=2
uint8 SOURCE_DEFAULT=3

string key
string value
# False when the value has no default; default_value is then empty.
bool has_default
string default_value
uint8 source
