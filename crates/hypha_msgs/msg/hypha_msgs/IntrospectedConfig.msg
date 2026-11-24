# One configuration value of a node, as text.
uint8 SOURCE_PARAMETER=0
uint8 SOURCE_APPLICATION=1
uint8 SOURCE_COMMAND_LINE=2
uint8 SOURCE_DEFAULT=3
# A key set on purpose and not read (the file's `unused` list, or ignore() in code);
# value holds the reason.
uint8 SOURCE_IGNORED=4

string key
string value
# False when the value has no default; default_value is then empty.
bool has_default
string default_value
uint8 source
