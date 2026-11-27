# One schema of a topic: its stream's schema hash, its message type and the MD5
# of its definition. A topic carries one stream for each MD5. data_type and md5
# are empty for a stream whose endpoints gave no definition: schema_hash is then
# the first eight bytes of the MD5, little-endian.

uint64 schema_hash
string data_type
string md5
