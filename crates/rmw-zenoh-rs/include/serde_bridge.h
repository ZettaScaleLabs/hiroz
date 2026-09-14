#pragma once

#include <rosidl_typesupport_fastrtps_c/identifier.h>
#include <rosidl_typesupport_fastrtps_cpp/message_type_support.h>
#include <rosidl_typesupport_fastrtps_cpp/service_type_support.h>

#include <rosidl_typesupport_fastrtps_cpp/identifier.hpp>

#include "rust/cxx.h"

using c_void = void;

namespace serde_bridge {

const rosidl_message_type_support_t* get_message_typesupport(const rosidl_message_type_support_t* ts);

size_t get_serialized_size(const rosidl_message_type_support_t* ts, const void* ros_message);

bool serialize_message(const rosidl_message_type_support_t* ts, const void* ros_message, rust::Vec<uint8_t>& out);

bool deserialize_message(const rosidl_message_type_support_t* ts, const rust::Vec<uint8_t>& data, void* ros_message);

rust::String get_message_name(const rosidl_message_type_support_t* ts);

rust::String get_message_namespace(const rosidl_message_type_support_t* ts);

const rosidl_service_type_support_t* get_service_typesupport(const rosidl_service_type_support_t* ts);

const rosidl_message_type_support_t* get_request_type_support(const rosidl_service_type_support_t* ts);

const rosidl_message_type_support_t* get_response_type_support(const rosidl_service_type_support_t* ts);

// Type hash support (returns null stub on Humble)
const rosidl_type_hash_t* get_service_type_hash(const rosidl_service_type_support_t* ts);

};  // namespace serde_bridge
