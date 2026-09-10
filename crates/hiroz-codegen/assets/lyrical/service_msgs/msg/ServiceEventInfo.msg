uint8 REQUEST_SENT = 0
uint8 REQUEST_RECEIVED = 1
uint8 RESPONSE_SENT = 2
uint8 RESPONSE_RECEIVED = 3

# The type of event this message represents
uint8 event_type

# Timestamp for when the event occurred (sent or received time)
builtin_interfaces/Time stamp

# Unique identifier for the client that sent the service request
# Note, this is only unique for the current session.
# The size here has to match the size of rmw_dds_common/msg/Gid,
# but unfortunately we cannot use that message directly due to a
# circular dependency.
char[16] client_gid

# Sequence number for the request
# Combined with the client ID, this creates a unique ID for the service transaction
int64 sequence_number
