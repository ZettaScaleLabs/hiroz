# All available logger levels; these correspond to the enum in rcutils/logger.h

uint8 LOG_LEVEL_UNKNOWN = 0
uint8 LOG_LEVEL_DEBUG = 10
uint8 LOG_LEVEL_INFO = 20
uint8 LOG_LEVEL_WARN = 30
uint8 LOG_LEVEL_ERROR = 40
uint8 LOG_LEVEL_FATAL = 50

# The logger name.
string name

# The logger level
uint32 level
