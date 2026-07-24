# A map of logger name and its logging severity level.

# The logger name '' will set the default logging severity level for a given node.
string name

# The logging severity level.
# Supported logging severity level values from rcl_interfaces/msg/Log:
#   DEBUG=10 INFO=20 WARN=30 ERROR=40 FATAL=50
uint32 level
