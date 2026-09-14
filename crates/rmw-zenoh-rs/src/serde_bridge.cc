#include "serde_bridge.h"

#include <fastcdr/Cdr.h>
#include <fastcdr/FastBuffer.h>
#include <rmw/error_handling.h>
#include <rosidl_typesupport_fastrtps_c/identifier.h>
#include <rosidl_typesupport_interface/macros.h>

#include <rosidl_typesupport_fastrtps_cpp/identifier.hpp>

namespace serde_bridge {

// In order to avoid depending on fastcdr libirary in Rust, we don't exposes message_type_support_callbacks_t here.
const rosidl_message_type_support_t* get_message_typesupport(const rosidl_message_type_support_t* ts) {
    if (!ts) {
        return nullptr;
    }
    auto type_support = get_message_typesupport_handle(ts, rosidl_typesupport_fastrtps_c__identifier);
    if (!type_support) {
        // Clear error from first attempt before trying second type support
        rmw_reset_error();
        type_support = get_message_typesupport_handle(ts, rosidl_typesupport_fastrtps_cpp::typesupport_identifier);
    }

    if (!type_support) {
        return nullptr;
    }
    return type_support;
}

size_t get_serialized_size(const rosidl_message_type_support_t* ts, const void* ros_message) {
    const message_type_support_callbacks_t* callbacks = static_cast<const message_type_support_callbacks_t*>(ts->data);
    return 4 + callbacks->get_serialized_size(ros_message);
}

bool serialize_message(const rosidl_message_type_support_t* ts, const void* ros_message, rust::Vec<uint8_t>& out) {
    if (!ts || !ros_message) {
        return false;
    }

    auto callbacks = static_cast<const message_type_support_callbacks_t*>(ts->data);

    eprosima::fastcdr::FastBuffer buffer(reinterpret_cast<char*>(out.data()), out.size());
    eprosima::fastcdr::Cdr ser(buffer, eprosima::fastcdr::Cdr::DEFAULT_ENDIAN, eprosima::fastcdr::CdrVersion::DDS_CDR);

    ser.serialize_encapsulation();
    return callbacks->cdr_serialize(ros_message, ser);
}

bool deserialize_message(const rosidl_message_type_support_t* ts, const rust::Vec<uint8_t>& data, void* ros_message) {
    if (!ts || !ros_message) {
        return false;
    }

    try {
        auto callbacks = static_cast<const message_type_support_callbacks_t*>(ts->data);

        eprosima::fastcdr::FastBuffer buffer(reinterpret_cast<char*>(const_cast<uint8_t*>(data.data())), data.size());
        eprosima::fastcdr::Cdr cdr(buffer);
        cdr.read_encapsulation();

        return callbacks->cdr_deserialize(cdr, ros_message);
    } catch (...) {
        return false;
    }
}

rust::String get_message_name(const rosidl_message_type_support_t* ts) {
    auto callbacks = static_cast<const message_type_support_callbacks_t*>(ts->data);
    return rust::String(callbacks->message_name_);
}

rust::String get_message_namespace(const rosidl_message_type_support_t* ts) {
    auto callbacks = static_cast<const message_type_support_callbacks_t*>(ts->data);
    return rust::String(callbacks->message_namespace_);
}

const rosidl_service_type_support_t* get_service_typesupport(const rosidl_service_type_support_t* ts) {
    if (!ts) {
        return nullptr;
    }
    auto type_support = get_service_typesupport_handle(ts, rosidl_typesupport_fastrtps_c__identifier);
    if (!type_support) {
        // Clear error from first attempt before trying second type support
        rmw_reset_error();
        type_support = get_service_typesupport_handle(ts, rosidl_typesupport_fastrtps_cpp::typesupport_identifier);
    }

    if (!type_support) {
        return nullptr;
    }
    return type_support;
}

const rosidl_message_type_support_t* get_request_type_support(const rosidl_service_type_support_t* ts) {
    auto callbacks = static_cast<const service_type_support_callbacks_t*>(ts->data);
    return callbacks->request_members_;
}

const rosidl_message_type_support_t* get_response_type_support(const rosidl_service_type_support_t* ts) {
    auto callbacks = static_cast<const service_type_support_callbacks_t*>(ts->data);
    return callbacks->response_members_;
}

const rosidl_type_hash_t* get_service_type_hash(const rosidl_service_type_support_t* ts) {
    if (!ts || !ts->get_type_hash_func) {
        return nullptr;
    }
    return ts->get_type_hash_func(ts);
}

}  // namespace serde_bridge
