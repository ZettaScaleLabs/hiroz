# Represents the original source of a ROS 2 interface definition.

# ROS interface type name, in PACKAGE/NAMESPACE/TYPENAME format.
string type_name

# The type of the original source file, typically matching the file extension.
# Well-known encodings: "idl", "msg", "srv", "action", "dynamic", "implicit".
# "dynamic" specifies a type created programmatically by a user, thus having no source.
# "implicit" specifies a type created automatically as a subtype of a
# complex type (service or action) - such as the request message for a service.
# Implicit types will have no contents, the full source will be available on the parent srv/action.
string encoding

# Dumped contents of the interface definition source file.
# If `encoding` is "dynamic" or "implicit", this field will be empty.
string raw_file_contents
