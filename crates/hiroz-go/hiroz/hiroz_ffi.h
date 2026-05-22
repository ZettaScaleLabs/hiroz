#ifndef HIROZ_FFI_H
#define HIROZ_FFI_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#define hiroz_ZENOH_EVENT_ID_MAX 11

/**
 * Default depth for KEEP_LAST when SYSTEM_DEFAULT (depth=0) is used
 * This matches ROS 2 and rmw_zenoh_cpp behavior
 */
#define hiroz_DEFAULT_HISTORY_DEPTH 10

/**
 * Default shared memory pool size (10 MB).
 */
#define hiroz_DEFAULT_SHM_POOL_SIZE ((10 * 1024) * 1024)

/**
 * Default message size threshold for using SHM (512 bytes).
 *
 * Messages smaller than this will use regular memory allocation.
 * Matches rmw_zenoh_cpp default for compatibility.
 */
#define hiroz_DEFAULT_SHM_THRESHOLD 512

/**
 * Opaque action client handle for FFI
 */
typedef struct hiroz_action_client_t hiroz_action_client_t;

/**
 * Opaque action server handle for FFI
 */
typedef struct hiroz_action_server_t hiroz_action_server_t;

/**
 * Opaque goal handle for FFI (client-side)
 */
typedef struct hiroz_goal_handle_t hiroz_goal_handle_t;

/**
 * Opaque service server handle for FFI
 */
typedef struct hiroz_service_server_t hiroz_service_server_t;

/**
 * Represents a QoS duration in seconds and nanoseconds.
 *
 * This is distinct from [`std::time::Duration`] and is used exclusively for
 * configuring QoS deadline, lifespan, and liveliness lease duration.
 * Use [`QosDuration::INFINITE`] (the default) to disable a QoS time constraint.
 */
typedef struct hiroz_QosDuration hiroz_QosDuration;

/**
 * Raw publisher for FFI (no type parameters)
 */
typedef struct hiroz_RawPublisher hiroz_RawPublisher;

/**
 * Raw service client for FFI (no type parameters)
 */
typedef struct hiroz_RawServiceClient hiroz_RawServiceClient;

/**
 * Raw subscriber wrapper that keeps the zenoh subscriber alive
 */
typedef struct hiroz_RawSubscriber hiroz_RawSubscriber;

/**
 * A live hiroz context backed by an open Zenoh session.
 *
 * `ZContext` is the root object for all hiroz communication. Create one with
 * [`ZContextBuilder`] and use it to create [`ZNode`](crate::node::ZNode)s.
 *
 * # Example
 *
 * ```rust,ignore
 * use hiroz::prelude::*;
 *
 * let ctx = ZContextBuilder::default().build()?;
 * let node = ctx.create_node("my_node").build()?;
 * ```
 */
typedef struct hiroz_ZContext hiroz_ZContext;

/**
 * A ROS 2-style node: a named participant that owns publishers, subscribers,
 * service clients, service servers, and action clients/servers.
 *
 * Create a node via [`ZContext::create_node`](crate::context::ZContext::create_node):
 *
 * ```rust,ignore
 * use hiroz::prelude::*;
 *
 * let ctx = ZContextBuilder::default().build()?;
 * let node = ctx.create_node("my_node").build()?;
 * ```
 */
typedef struct hiroz_ZNode hiroz_ZNode;

/**
 * Opaque node handle for FFI
 */
typedef struct hiroz_node_t {
  struct hiroz_ZNode *inner;
} hiroz_node_t;

/**
 * Callback type for goal acceptance.
 * Returns 1 for accept, 0 for reject.
 */
typedef int32_t (*hiroz_ActionGoalCallback)(uintptr_t user_data,
                                            const uint8_t *goal_data,
                                            uintptr_t goal_len);

/**
 * Callback type for goal execution.
 * Must write result bytes and return 0 on success.
 * The goal_id (16 bytes) identifies this specific goal for feedback publishing.
 */
typedef int32_t (*hiroz_ActionExecuteCallback)(uintptr_t user_data,
                                               const uint8_t *goal_id,
                                               const uint8_t *goal_data,
                                               uintptr_t goal_len,
                                               uint8_t **result_data,
                                               uintptr_t *result_len);

/**
 * Opaque context handle for FFI
 */
typedef struct hiroz_context_t {
  struct hiroz_ZContext *inner;
} hiroz_context_t;

/**
 * Configuration struct for context creation
 */
typedef struct hiroz_context_config_t {
  uint32_t domain_id;
  /**
   * Path to a Zenoh JSON5 config file (nullable)
   */
  const char *config_file;
  /**
   * Array of connect endpoint strings (nullable)
   */
  const char *const *connect_endpoints;
  /**
   * Number of connect endpoints
   */
  uintptr_t connect_endpoints_count;
  /**
   * Zenoh mode: "peer", "client", "router" (nullable = default)
   */
  const char *mode;
  /**
   * Whether to disable multicast scouting
   */
  bool disable_multicast_scouting;
  /**
   * Whether to connect to local zenohd on tcp/127.0.0.1:7447
   */
  bool connect_to_local_zenohd;
  /**
   * JSON string for arbitrary Zenoh config overrides (nullable)
   * Format: JSON object with dotted keys, e.g. {"scouting/multicast/enabled": false}
   */
  const char *json_config;
  /**
   * Array of remap rule strings in "from:=to" format (nullable)
   */
  const char *const *remap_rules;
  /**
   * Number of remap rules
   */
  uintptr_t remap_rules_count;
  /**
   * Whether to enable logging
   */
  bool enable_logging;
} hiroz_context_config_t;

/**
 * Topic info returned to FFI callers
 */
typedef struct hiroz_topic_info_t {
  char *name;
  char *type_name;
} hiroz_topic_info_t;

/**
 * Node info returned to FFI callers
 */
typedef struct hiroz_node_info_t {
  char *name;
  char *namespace_;
} hiroz_node_info_t;

/**
 * Service info returned to FFI callers
 */
typedef struct hiroz_service_info_t {
  char *name;
  char *type_name;
} hiroz_service_info_t;

/**
 * Node configuration for FFI
 */
typedef struct hiroz_node_config_t {
  const char *name;
  const char *namespace_;
  bool enable_type_description_service;
} hiroz_node_config_t;

/**
 * Opaque publisher handle for FFI
 */
typedef struct hiroz_publisher_t {
  struct hiroz_RawPublisher *inner;
} hiroz_publisher_t;

/**
 * C-compatible QoS profile for FFI
 */
typedef struct hiroz_qos_profile_t {
  /**
   * 0 = Reliable (default), 1 = BestEffort
   */
  int32_t reliability;
  /**
   * 0 = Volatile (default), 1 = TransientLocal
   */
  int32_t durability;
  /**
   * 0 = KeepLast (default), 1 = KeepAll
   */
  int32_t history;
  /**
   * Depth for KeepLast (default: 10, ignored for KeepAll)
   */
  int32_t history_depth;
  uint64_t deadline_sec;
  uint64_t deadline_nsec;
  uint64_t lifespan_sec;
  uint64_t lifespan_nsec;
  /**
   * 0 = Automatic (default), 1 = ManualByNode, 2 = ManualByTopic
   */
  int32_t liveliness;
  uint64_t liveliness_lease_sec;
  uint64_t liveliness_lease_nsec;
} hiroz_qos_profile_t;

/**
 * Opaque service client handle for FFI
 */
typedef struct hiroz_service_client_t {
  struct hiroz_RawServiceClient *inner;
} hiroz_service_client_t;

/**
 * Callback type for service requests.
 * Called with (user_data, request bytes, request len, out response bytes, out response len).
 * Must return 0 on success, non-zero on error.
 */
typedef int32_t (*hiroz_ServiceCallback)(uintptr_t user_data,
                                         const uint8_t *request_data,
                                         uintptr_t request_len,
                                         uint8_t **response_data,
                                         uintptr_t *response_len);

/**
 * Opaque subscriber handle for FFI
 */
typedef struct hiroz_subscriber_t {
  struct hiroz_RawSubscriber *inner;
} hiroz_subscriber_t;

/**
 * Callback type for receiving messages
 */
typedef void (*hiroz_MessageCallback)(uintptr_t user_data, const uint8_t *data, uintptr_t len);



#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * Create an action client
 */
struct hiroz_action_client_t *hiroz_action_client_create(struct hiroz_node_t *node,
                                                         const char *action_name,
                                                         const char *action_type_name,
                                                         const char *goal_type_name,
                                                         const char *goal_type_hash,
                                                         const char *result_type_name,
                                                         const char *result_type_hash,
                                                         const char *feedback_type_name,
                                                         const char *feedback_type_hash);

/**
 * Send a goal to an action server.
 * On success, writes the goal_id (16 bytes) and creates a goal handle.
 */
int32_t hiroz_action_client_send_goal(struct hiroz_action_client_t *client_handle,
                                      const uint8_t *goal_data,
                                      uintptr_t goal_len,
                                      uint8_t (*goal_id)[16],
                                      struct hiroz_goal_handle_t **handle);

/**
 * Get result for a goal
 */
int32_t hiroz_action_client_get_result(struct hiroz_goal_handle_t *goal_handle,
                                       uint8_t **result_data,
                                       uintptr_t *result_len);

/**
 * Cancel a goal
 */
int32_t hiroz_action_client_cancel_goal(struct hiroz_goal_handle_t *goal_handle);

/**
 * Destroy an action client
 */
int32_t hiroz_action_client_destroy(struct hiroz_action_client_t *client);

extern void free(void *ptr);

/**
 * Create an action server.
 * Spawns a background thread that polls for goals, calls the goal callback,
 * and if accepted, calls the execute callback.
 */
struct hiroz_action_server_t *hiroz_action_server_create(struct hiroz_node_t *node,
                                                         const char *action_name,
                                                         const char *action_type_name,
                                                         const char *goal_type_name,
                                                         const char *goal_type_hash,
                                                         const char *result_type_name,
                                                         const char *result_type_hash,
                                                         const char *feedback_type_name,
                                                         const char *feedback_type_hash,
                                                         hiroz_ActionGoalCallback goal_callback,
                                                         hiroz_ActionExecuteCallback execute_callback,
                                                         uintptr_t user_data);

/**
 * Publish feedback for a goal
 */
int32_t hiroz_action_server_publish_feedback(struct hiroz_action_server_t *server_handle,
                                             const uint8_t (*goal_id)[16],
                                             const uint8_t *feedback_data,
                                             uintptr_t feedback_len);

/**
 * Mark a goal as succeeded
 */
int32_t hiroz_action_server_succeed(struct hiroz_action_server_t *server_handle,
                                    const uint8_t (*goal_id)[16],
                                    const uint8_t *result_data,
                                    uintptr_t result_len);

/**
 * Mark a goal as aborted
 */
int32_t hiroz_action_server_abort(struct hiroz_action_server_t *server_handle,
                                  const uint8_t (*goal_id)[16],
                                  const uint8_t *result_data,
                                  uintptr_t result_len);

/**
 * Mark a goal as canceled
 */
int32_t hiroz_action_server_canceled(struct hiroz_action_server_t *server_handle,
                                     const uint8_t (*goal_id)[16],
                                     const uint8_t *result_data,
                                     uintptr_t result_len);

/**
 * Check whether a cancel has been requested for the given goal.
 * Returns 1 if cancel was requested, 0 otherwise.
 */
int32_t hiroz_action_server_is_cancel_requested(struct hiroz_action_server_t *server_handle,
                                                const uint8_t (*goal_id)[16]);

/**
 * Destroy an action server
 */
int32_t hiroz_action_server_destroy(struct hiroz_action_server_t *server);

/**
 * Destroy a goal handle
 */
int32_t hiroz_goal_handle_destroy(struct hiroz_goal_handle_t *handle);

/**
 * Create a new hiroz context with default config (convenience)
 *
 * # Safety
 * Must be called from a valid thread. The returned pointer must be freed
 * with `hiroz_context_destroy`.
 */
struct hiroz_context_t *hiroz_context_create(uint32_t domain_id);

/**
 * Create a new hiroz context with full configuration
 *
 * # Safety
 * `config` must be a valid pointer to a `CContextConfig` struct, or null.
 * String pointers within the config must be valid null-terminated C strings or null.
 * Array pointers must be valid for the specified count, or null with count 0.
 * The returned pointer must be freed with `hiroz_context_destroy`.
 */
struct hiroz_context_t *hiroz_context_create_with_config(const struct hiroz_context_config_t *config);

/**
 * Shutdown and free context
 *
 * # Safety
 * `ctx` must be a valid pointer returned by `hiroz_context_create` or
 * `hiroz_context_create_with_config`, or null.
 */
int32_t hiroz_context_destroy(struct hiroz_context_t *ctx);

/**
 * Get all topic names and types
 *
 * # Safety
 * `ctx` must be a valid context pointer. `out_topics` and `out_count` must be
 * valid non-null pointers. The returned array must be freed with `hiroz_graph_free_topics`.
 */
int32_t hiroz_graph_get_topic_names_and_types(struct hiroz_context_t *ctx,
                                              struct hiroz_topic_info_t **out_topics,
                                              uintptr_t *out_count);

/**
 * Free topic info array
 *
 * # Safety
 * `topics` must be a pointer returned by `hiroz_graph_get_topic_names_and_types`,
 * or null. `count` must match the count returned by that function.
 */
void hiroz_graph_free_topics(struct hiroz_topic_info_t *topics, uintptr_t count);

/**
 * Get all node names and namespaces
 *
 * # Safety
 * `ctx` must be a valid context pointer. `out_nodes` and `out_count` must be
 * valid non-null pointers. The returned array must be freed with `hiroz_graph_free_nodes`.
 */
int32_t hiroz_graph_get_node_names(struct hiroz_context_t *ctx,
                                   struct hiroz_node_info_t **out_nodes,
                                   uintptr_t *out_count);

/**
 * Free node info array
 *
 * # Safety
 * `nodes` must be a pointer returned by `hiroz_graph_get_node_names`,
 * or null. `count` must match the count returned by that function.
 */
void hiroz_graph_free_nodes(struct hiroz_node_info_t *nodes, uintptr_t count);

/**
 * Get all service names and types
 *
 * # Safety
 * `ctx` must be a valid context pointer. `out_services` and `out_count` must be
 * valid non-null pointers. The returned array must be freed with `hiroz_graph_free_services`.
 */
int32_t hiroz_graph_get_service_names_and_types(struct hiroz_context_t *ctx,
                                                struct hiroz_service_info_t **out_services,
                                                uintptr_t *out_count);

/**
 * Free service info array
 *
 * # Safety
 * `services` must be a pointer returned by `hiroz_graph_get_service_names_and_types`,
 * or null. `count` must match the count returned by that function.
 */
void hiroz_graph_free_services(struct hiroz_service_info_t *services, uintptr_t count);

/**
 * Check if a node exists in the graph
 *
 * # Safety
 * `ctx` must be a valid context pointer. `name` must be a valid C string.
 * `namespace` may be null (defaults to "/").
 * Returns 1 if found, 0 if not found, or negative error code.
 */
int32_t hiroz_graph_node_exists(struct hiroz_context_t *ctx,
                                const char *name,
                                const char *namespace_);

/**
 * Create a new node (simple API)
 *
 * # Safety
 * `ctx` must be a valid context pointer. `name` must be a valid C string.
 * `namespace` may be null. The returned pointer must be freed with `hiroz_node_destroy`.
 */
struct hiroz_node_t *hiroz_node_create(struct hiroz_context_t *ctx,
                                       const char *name,
                                       const char *namespace_);

/**
 * Create a new node with full configuration
 *
 * # Safety
 * `ctx` must be a valid context pointer. `config` must be a valid pointer to
 * `CNodeConfig` or null. String fields in config must be valid C strings or null.
 */
struct hiroz_node_t *hiroz_node_create_with_config(struct hiroz_context_t *ctx,
                                                   const struct hiroz_node_config_t *config);

/**
 * Destroy a node
 *
 * # Safety
 * `node` must be a valid pointer returned by `hiroz_node_create` or null.
 */
int32_t hiroz_node_destroy(struct hiroz_node_t *node);

/**
 * Create a publisher (default QoS)
 *
 * # Safety
 * `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
 * must be valid C strings. The returned pointer must be freed with `hiroz_publisher_destroy`.
 */
struct hiroz_publisher_t *hiroz_publisher_create(struct hiroz_node_t *node,
                                                 const char *topic,
                                                 const char *type_name,
                                                 const char *type_hash);

/**
 * Create a publisher with QoS profile
 *
 * # Safety
 * `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
 * must be valid C strings. `qos` may be null for default QoS.
 */
struct hiroz_publisher_t *hiroz_publisher_create_with_qos(struct hiroz_node_t *node,
                                                          const char *topic,
                                                          const char *type_name,
                                                          const char *type_hash,
                                                          const struct hiroz_qos_profile_t *qos);

/**
 * Publish raw bytes (already CDR serialized)
 *
 * # Safety
 * `pub_handle` must be a valid publisher pointer. `data` must be valid for `len` bytes.
 */
int32_t hiroz_publisher_publish(struct hiroz_publisher_t *pub_handle,
                                const uint8_t *data,
                                uintptr_t len);

/**
 * Destroy a publisher
 *
 * # Safety
 * `pub_handle` must be a valid publisher pointer or null.
 */
int32_t hiroz_publisher_destroy(struct hiroz_publisher_t *pub_handle);

/**
 * Serialize a message to CDR format
 * Input: type_name (C string), raw message bytes from Go
 * Output: CDR serialized bytes via out_ptr/out_len
 */
int32_t hiroz_serialize(const char *type_name,
                        const uint8_t *msg_data,
                        uintptr_t msg_len,
                        uint8_t **out_ptr,
                        uintptr_t *out_len);

/**
 * Deserialize CDR bytes to raw format for Go
 */
int32_t hiroz_deserialize(const char *type_name,
                          const uint8_t *cdr_data,
                          uintptr_t cdr_len,
                          uint8_t **out_ptr,
                          uintptr_t *out_len);

/**
 * Free bytes allocated by serialize/deserialize
 */
void hiroz_free_bytes(uint8_t *ptr, uintptr_t len);

/**
 * Create a service client
 */
struct hiroz_service_client_t *hiroz_service_client_create(struct hiroz_node_t *node,
                                                           const char *service_name,
                                                           const char *req_type_name,
                                                           const char *req_type_hash,
                                                           const char *resp_type_name,
                                                           const char *resp_type_hash);

/**
 * Call a service (synchronous with timeout).
 * Response bytes are allocated via Rust and must be freed with hiroz_free_bytes.
 */
int32_t hiroz_service_client_call(struct hiroz_service_client_t *client_handle,
                                  const uint8_t *request_data,
                                  uintptr_t request_len,
                                  uint8_t **response_data,
                                  uintptr_t *response_len,
                                  uint64_t timeout_ms);

/**
 * Destroy a service client
 */
int32_t hiroz_service_client_destroy(struct hiroz_service_client_t *client);

/**
 * Wait until at least one matching service server is visible in the graph,
 * or `timeout_ms` elapses. Returns 0 if ready, -10 (ServiceTimeout) on
 * timeout, -1 (NullPointer) if `client_handle` is null.
 */
int32_t hiroz_service_client_wait_for_service(struct hiroz_service_client_t *client_handle,
                                              uint64_t timeout_ms);

extern void free(void *ptr);

/**
 * Create a service server.
 * The server spawns a background thread that polls for incoming requests,
 * invokes the callback for each one, and sends the response.
 */
struct hiroz_service_server_t *hiroz_service_server_create(struct hiroz_node_t *node,
                                                           const char *service_name,
                                                           const char *req_type_name,
                                                           const char *req_type_hash,
                                                           const char *resp_type_name,
                                                           const char *resp_type_hash,
                                                           hiroz_ServiceCallback callback,
                                                           uintptr_t user_data);

/**
 * Destroy a service server
 */
int32_t hiroz_service_server_destroy(struct hiroz_service_server_t *server);

/**
 * Create a subscriber with callback (default QoS)
 *
 * # Safety
 * `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
 * must be valid C strings. `callback` must be a valid function pointer.
 * `user_data` is passed through to the callback.
 */
struct hiroz_subscriber_t *hiroz_subscriber_create(struct hiroz_node_t *node,
                                                   const char *topic,
                                                   const char *type_name,
                                                   const char *type_hash,
                                                   hiroz_MessageCallback callback,
                                                   uintptr_t user_data);

/**
 * Create a subscriber with callback and QoS profile
 *
 * # Safety
 * `node` must be a valid node pointer. `topic`, `type_name`, and `type_hash`
 * must be valid C strings. `callback` must be a valid function pointer.
 * `qos` may be null for default QoS.
 */
struct hiroz_subscriber_t *hiroz_subscriber_create_with_qos(struct hiroz_node_t *node,
                                                            const char *topic,
                                                            const char *type_name,
                                                            const char *type_hash,
                                                            hiroz_MessageCallback callback,
                                                            uintptr_t user_data,
                                                            const struct hiroz_qos_profile_t *qos);

/**
 * Destroy a subscriber
 *
 * # Safety
 * `sub` must be a valid subscriber pointer or null.
 */
int32_t hiroz_subscriber_destroy(struct hiroz_subscriber_t *sub);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* HIROZ_FFI_H */
