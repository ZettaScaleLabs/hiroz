# Represents a single field in a type.

# Name of the field.
string name
# Type of the field, including details about the type like length, nested name, etc.
FieldType type
# Literal default value of the field as a string, as it appeared in the original
# message description file, whether that be .msg/.srv/.action or .idl.
string default_value
