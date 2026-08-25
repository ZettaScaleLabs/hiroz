#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "rcutils/allocator.h"
#include "rcutils/macros.h"
#include "rcutils/types.h"
#include "rmw/publisher_options.h"
#include "rmw/qos_profiles.h"
#include "rmw/subscription_options.h"
#include "rmw/types.h"
#include "rosidl_runtime_c/message_type_support_struct.h"
#include "rosidl_runtime_c/service_type_support_struct.h"
#include "rust/cxx.h"

using c_void = void;

namespace rmw_bridge {

// RMW API functions - these will be implemented in Rust

extern "C" {

// Initialization and Shutdown
const char* rmw_get_implementation_identifier_bridge(void);

const char* rmw_get_serialization_format_bridge(void);

rmw_node_t* rmw_create_node_bridge(rmw_context_t* context, const char* name, const char* namespace_);

rmw_ret_t rmw_destroy_node_bridge(rmw_node_t* node);

// Publishers
rmw_publisher_t* rmw_create_publisher_bridge(const rmw_node_t* node, const rosidl_message_type_support_t* type_support,
                                             const char* topic_name, const rmw_qos_profile_t* qos_profile,
                                             const rmw_publisher_options_t* publisher_options);

rmw_ret_t rmw_destroy_publisher_bridge(rmw_node_t* node, rmw_publisher_t* publisher);

rmw_ret_t rmw_publish_bridge(const rmw_publisher_t* publisher, const void* ros_message,
                             rmw_publisher_allocation_t* allocation);

// Subscriptions
rmw_subscription_t* rmw_create_subscription_bridge(const rmw_node_t* node,
                                                   const rosidl_message_type_support_t* type_support,
                                                   const char* topic_name, const rmw_qos_profile_t* qos_policies,
                                                   const rmw_subscription_options_t* subscription_options);

rmw_ret_t rmw_destroy_subscription_bridge(rmw_node_t* node, rmw_subscription_t* subscription);

rmw_ret_t rmw_take_bridge(const rmw_subscription_t* subscription, void* ros_message, bool* taken,
                          rmw_subscription_allocation_t* allocation);

// Services
rmw_client_t* rmw_create_client_bridge(const rmw_node_t* node, const rosidl_service_type_support_t* type_support,
                                       const char* service_name, const rmw_qos_profile_t* qos_policies);

rmw_ret_t rmw_destroy_client_bridge(rmw_node_t* node, rmw_client_t* client);

rmw_service_t* rmw_create_service_bridge(const rmw_node_t* node, const rosidl_service_type_support_t* type_support,
                                         const char* service_name, const rmw_qos_profile_t* qos_profile);

rmw_ret_t rmw_destroy_service_bridge(rmw_node_t* node, rmw_service_t* service);

// Wait sets
rmw_wait_set_t* rmw_create_wait_set_bridge(rmw_context_t* context, size_t max_conditions);

rmw_ret_t rmw_destroy_wait_set_bridge(rmw_wait_set_t* wait_set);

rmw_ret_t rmw_wait_bridge(rmw_subscriptions_t* subscriptions, rmw_guard_conditions_t* guard_conditions,
                          rmw_services_t* services, rmw_clients_t* clients, rmw_events_t* events,
                          rmw_wait_set_t* wait_set, const rmw_time_t* wait_timeout);

// Guard conditions
rmw_guard_condition_t* rmw_create_guard_condition_bridge(rmw_context_t* context);

rmw_ret_t rmw_destroy_guard_condition_bridge(rmw_guard_condition_t* guard_condition);

rmw_ret_t rmw_trigger_guard_condition_bridge(const rmw_guard_condition_t* guard_condition);

// Graph queries
rmw_ret_t rmw_get_node_names_bridge(const rmw_node_t* node, rcutils_string_array_t* node_names,
                                    rcutils_string_array_t* node_namespaces);

// Add more functions as needed...

}  // extern "C"

}  // namespace rmw_bridge