# Represents the type of a field and related meta-data.

# A constant for each type supported according to:
#   http://design.ros2.org/articles/legacy_interface_definition.html
# and:
#   http://design.ros2.org/articles/idl_interface_definition.html
# Order is loosely coupled to the order of appearance in the IDL 4.2 spec:
#  https://www.omg.org/spec/IDL/4.2

# Layout of constants across the 0-255 decimal values in the uint8:
#
# - 000    : Reserved for "not set"
# - 001-048: Primitive types, strings, and reserved space for future primitive types
# - 049-096: Fixed sized array of primitive and string types
# - 097-144: Bounded Sequences of primitive and string types
# - 145-192: Unbounded Sequences of primitive and string types
# - 193-255: Reserved space for future array/sequence-like types

uint8 FIELD_TYPE_NOT_SET = 0

# Nested type defined in other .msg/.idl files.
uint8 FIELD_TYPE_NESTED_TYPE = 1

# Integer Types
uint8 FIELD_TYPE_INT8 = 2
uint8 FIELD_TYPE_UINT8 = 3
uint8 FIELD_TYPE_INT16 = 4
uint8 FIELD_TYPE_UINT16 = 5
uint8 FIELD_TYPE_INT32 = 6
uint8 FIELD_TYPE_UINT32 = 7
uint8 FIELD_TYPE_INT64 = 8
uint8 FIELD_TYPE_UINT64 = 9

# Floating-Point Types
uint8 FIELD_TYPE_FLOAT = 10
uint8 FIELD_TYPE_DOUBLE = 11
uint8 FIELD_TYPE_LONG_DOUBLE = 12

# Char and WChar Types
uint8 FIELD_TYPE_CHAR = 13
uint8 FIELD_TYPE_WCHAR = 14

# Boolean Type
uint8 FIELD_TYPE_BOOLEAN = 15

# Byte/Octet Type
uint8 FIELD_TYPE_BYTE = 16

# String Types
uint8 FIELD_TYPE_STRING = 17
uint8 FIELD_TYPE_WSTRING = 18

# Fixed String Types
uint8 FIELD_TYPE_FIXED_STRING = 19
uint8 FIELD_TYPE_FIXED_WSTRING = 20

# Bounded String Types
uint8 FIELD_TYPE_BOUNDED_STRING = 21
uint8 FIELD_TYPE_BOUNDED_WSTRING = 22

# Fixed Sized Array Types
uint8 FIELD_TYPE_NESTED_TYPE_ARRAY = 49
uint8 FIELD_TYPE_INT8_ARRAY = 50
uint8 FIELD_TYPE_UINT8_ARRAY = 51
uint8 FIELD_TYPE_INT16_ARRAY = 52
uint8 FIELD_TYPE_UINT16_ARRAY = 53
uint8 FIELD_TYPE_INT32_ARRAY = 54
uint8 FIELD_TYPE_UINT32_ARRAY = 55
uint8 FIELD_TYPE_INT64_ARRAY = 56
uint8 FIELD_TYPE_UINT64_ARRAY = 57
uint8 FIELD_TYPE_FLOAT_ARRAY = 58
uint8 FIELD_TYPE_DOUBLE_ARRAY = 59
uint8 FIELD_TYPE_LONG_DOUBLE_ARRAY = 60
uint8 FIELD_TYPE_CHAR_ARRAY = 61
uint8 FIELD_TYPE_WCHAR_ARRAY = 62
uint8 FIELD_TYPE_BOOLEAN_ARRAY = 63
uint8 FIELD_TYPE_BYTE_ARRAY = 64
uint8 FIELD_TYPE_STRING_ARRAY = 65
uint8 FIELD_TYPE_WSTRING_ARRAY = 66
uint8 FIELD_TYPE_FIXED_STRING_ARRAY = 67
uint8 FIELD_TYPE_FIXED_WSTRING_ARRAY = 68
uint8 FIELD_TYPE_BOUNDED_STRING_ARRAY = 69
uint8 FIELD_TYPE_BOUNDED_WSTRING_ARRAY = 70

# Bounded Sequence Types
uint8 FIELD_TYPE_NESTED_TYPE_BOUNDED_SEQUENCE = 97
uint8 FIELD_TYPE_INT8_BOUNDED_SEQUENCE = 98
uint8 FIELD_TYPE_UINT8_BOUNDED_SEQUENCE = 99
uint8 FIELD_TYPE_INT16_BOUNDED_SEQUENCE = 100
uint8 FIELD_TYPE_UINT16_BOUNDED_SEQUENCE = 101
uint8 FIELD_TYPE_INT32_BOUNDED_SEQUENCE = 102
uint8 FIELD_TYPE_UINT32_BOUNDED_SEQUENCE = 103
uint8 FIELD_TYPE_INT64_BOUNDED_SEQUENCE = 104
uint8 FIELD_TYPE_UINT64_BOUNDED_SEQUENCE = 105
uint8 FIELD_TYPE_FLOAT_BOUNDED_SEQUENCE = 106
uint8 FIELD_TYPE_DOUBLE_BOUNDED_SEQUENCE = 107
uint8 FIELD_TYPE_LONG_DOUBLE_BOUNDED_SEQUENCE = 108
uint8 FIELD_TYPE_CHAR_BOUNDED_SEQUENCE = 109
uint8 FIELD_TYPE_WCHAR_BOUNDED_SEQUENCE = 110
uint8 FIELD_TYPE_BOOLEAN_BOUNDED_SEQUENCE = 111
uint8 FIELD_TYPE_BYTE_BOUNDED_SEQUENCE = 112
uint8 FIELD_TYPE_STRING_BOUNDED_SEQUENCE = 113
uint8 FIELD_TYPE_WSTRING_BOUNDED_SEQUENCE = 114
uint8 FIELD_TYPE_FIXED_STRING_BOUNDED_SEQUENCE = 115
uint8 FIELD_TYPE_FIXED_WSTRING_BOUNDED_SEQUENCE = 116
uint8 FIELD_TYPE_BOUNDED_STRING_BOUNDED_SEQUENCE = 117
uint8 FIELD_TYPE_BOUNDED_WSTRING_BOUNDED_SEQUENCE = 118

# Unbounded Sequence Types
uint8 FIELD_TYPE_NESTED_TYPE_UNBOUNDED_SEQUENCE = 145
uint8 FIELD_TYPE_INT8_UNBOUNDED_SEQUENCE = 146
uint8 FIELD_TYPE_UINT8_UNBOUNDED_SEQUENCE = 147
uint8 FIELD_TYPE_INT16_UNBOUNDED_SEQUENCE = 148
uint8 FIELD_TYPE_UINT16_UNBOUNDED_SEQUENCE = 149
uint8 FIELD_TYPE_INT32_UNBOUNDED_SEQUENCE = 150
uint8 FIELD_TYPE_UINT32_UNBOUNDED_SEQUENCE = 151
uint8 FIELD_TYPE_INT64_UNBOUNDED_SEQUENCE = 152
uint8 FIELD_TYPE_UINT64_UNBOUNDED_SEQUENCE = 153
uint8 FIELD_TYPE_FLOAT_UNBOUNDED_SEQUENCE = 154
uint8 FIELD_TYPE_DOUBLE_UNBOUNDED_SEQUENCE = 155
uint8 FIELD_TYPE_LONG_DOUBLE_UNBOUNDED_SEQUENCE = 156
uint8 FIELD_TYPE_CHAR_UNBOUNDED_SEQUENCE = 157
uint8 FIELD_TYPE_WCHAR_UNBOUNDED_SEQUENCE = 158
uint8 FIELD_TYPE_BOOLEAN_UNBOUNDED_SEQUENCE = 159
uint8 FIELD_TYPE_BYTE_UNBOUNDED_SEQUENCE = 160
uint8 FIELD_TYPE_STRING_UNBOUNDED_SEQUENCE = 161
uint8 FIELD_TYPE_WSTRING_UNBOUNDED_SEQUENCE = 162
uint8 FIELD_TYPE_FIXED_STRING_UNBOUNDED_SEQUENCE = 163
uint8 FIELD_TYPE_FIXED_WSTRING_UNBOUNDED_SEQUENCE = 164
uint8 FIELD_TYPE_BOUNDED_STRING_UNBOUNDED_SEQUENCE = 165
uint8 FIELD_TYPE_BOUNDED_WSTRING_UNBOUNDED_SEQUENCE = 166

# Identifying number for the type of the field, using one of the above constants.
uint8 type_id 0

# Only used when the type is an array or a bounded sequence.
# In the case of an array, this is the fixed capacity of the array.
# In the case of a bounded sequence, this is the maximum capacity of the sequence.
# In all other cases this field is unused.
uint64 capacity

# Only used when the type is a fixed or bounded string/wstring, or a array/sequence of those.
# In the case of a fixed string/wstring, it is the fixed length of the string.
# In the case of a bounded string/wstring, it is the maximum capacity of the string.
# In the case of an array/sequence of fixed string/wstring, it is the fixed length of the strings.
# In the case of an array/sequence of bounded string/wstring, it is the maximum capacity of the strings.
# It is not currently possible to have different string capacities per element in the array/sequence.
uint64 string_capacity

# Only used when the type is a nested type or array/sequence of nested types.
# This is limited to 255 characters.
# TODO(wjwwood): this 255 character limit was chosen due to this being the limit
#   for DDSI-RTPS based middlewares, which is the most commonly used right now.
#   We lack a ROS 2 specific limit in our design documents, but we should update
#   this and/or link to the design doc when that is available.
string<=255 nested_type_name
