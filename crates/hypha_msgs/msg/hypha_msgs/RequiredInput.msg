# One input that an input set requires, as node health judged it.

uint8 USABLE=0
uint8 NEVER_ARRIVED=1
uint8 STALE=2

string topic
uint8 verdict
uint64 age_nanos
bool stagnant
bool ignored
uint64 dropped
uint64 rejected
uint64 undecodable
