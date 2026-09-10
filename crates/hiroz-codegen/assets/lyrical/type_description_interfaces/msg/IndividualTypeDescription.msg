# Represents a single type, without the types it references, if any.

# Name of the type.
# This is limited to 255 characters.
# TODO(wjwwood): this 255 character limit was chosen due to this being the limit
#   for DDSI-RTPS based middlewares, which is the most commonly used right now.
#   We lack a ROS 2 specific limit in our design documents, but we should update
#   this and/or link to the design doc when that is available.
string<=255 type_name
# Fields of the type.
Field[] fields
