#ifndef HIROZ_FFI_H
#define HIROZ_FFI_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#define hiroz_ZENOH_EVENT_ID_MAX 11

/**
 * Default depth for `KeepLast` when SYSTEM_DEFAULT (depth=0) is used.
 * Matches rclcpp's default of 10. Note this is distinct from
 * [`KEEP_ALL_CACHE_DEPTH`] below, which is the rmw_zenoh-aligned cap
 * applied to `KeepAll` when mapped to zenoh-ext's `cache.max_samples`.
 */
#define hiroz_DEFAULT_HISTORY_DEPTH 10

/**
 * Cache/history depth used when a TransientLocal endpoint carries
 * `QosHistory::KeepAll` (no inherent depth) or `KeepLast(0)`.
 *
 * Matches rmw_zenoh_cpp's `RMW_ZENOH_DEFAULT_HISTORY_DEPTH = 42`
 * (`rmw_zenoh_cpp/src/detail/qos.cpp:27`), which is the value
 * rmw_zenoh's `best_available_qos` substitutes for a zero-valued
 * `qos.depth` before passing it to
 * `AdvancedPublisherOptions::CacheOptions::max_samples`.
 *
 * This is intentionally a *finite* cap that mirrors rmw_zenoh's
 * pragmatic behaviour rather than the DDS KEEP_ALL spec's "keep
 * everything" semantics.
 */
#define hiroz_KEEP_ALL_CACHE_DEPTH 42

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
 * Constant for ListParameters: recursively get parameters with unlimited depth.
 */
#define hiroz_DEPTH_RECURSIVE 0

#define hiroz_NOT_SET 0

#define hiroz_BOOL 1

#define hiroz_INTEGER 2

#define hiroz_DOUBLE 3

#define hiroz_STRING 4

#define hiroz_BYTE_ARRAY 5

#define hiroz_BOOL_ARRAY 6

#define hiroz_INTEGER_ARRAY 7

#define hiroz_DOUBLE_ARRAY 8

#define hiroz_STRING_ARRAY 9

typedef struct hiroz_action_client_t hiroz_action_client_t;

typedef struct hiroz_CActionRequest hiroz_CActionRequest;

typedef struct hiroz_action_server_t hiroz_action_server_t;

/**
 * An independent in-flight query. Destroying it cancels the transport query.
 * The creating client may be destroyed once this handle has been returned.
 */
typedef struct hiroz_service_call_t hiroz_service_call_t;

/**
 * A request owns its query independently of the server that received it.
 */
typedef struct hiroz_service_request_t hiroz_service_request_t;

/**
 * Opaque service server handle for FFI
 */
typedef struct hiroz_service_server_t hiroz_service_server_t;

/**
 * Opaque subscriber handle for FFI
 */
typedef struct hiroz_subscriber_t hiroz_subscriber_t;

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

typedef struct hiroz_CActionQos {
  struct hiroz_qos_profile_t goal;
  struct hiroz_qos_profile_t result;
  struct hiroz_qos_profile_t cancel;
  struct hiroz_qos_profile_t feedback;
  struct hiroz_qos_profile_t status;
} hiroz_CActionQos;

/**
 * kind=0 is a complete FeedbackMessage, kind=1 a complete GoalStatusArray.
 */
typedef void (*hiroz_ActionEventCallback)(uintptr_t, int32_t, const uint8_t*, uintptr_t);

/**
 * Called once after all native references to the foreign event data are released.
 */
typedef void (*hiroz_ActionEventDropCallback)(uintptr_t);

typedef int32_t (*hiroz_ActionGoalCallback)(uintptr_t, const uint8_t*, const uint8_t*, uintptr_t);

typedef int32_t (*hiroz_ActionExecuteCallback)(uintptr_t,
                                               const uint8_t*,
                                               const uint8_t*,
                                               uintptr_t,
                                               uint8_t**,
                                               uintptr_t*);

typedef int32_t (*hiroz_ActionCancelCallback)(uintptr_t, const uint8_t*);

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
  /**
   * Default namespace inherited by nodes created from this context (nullable).
   * Added after all pre-existing fields to preserve ABI compatibility.
   */
  const char *namespace_;
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
 * Correlation information attached to a service request.
 */
typedef struct hiroz_service_request_id_t {
  int64_t sequence_number;
  uint8_t writer_gid[16];
} hiroz_service_request_id_t;

/**
 * Callback type for receiving messages
 */
typedef void (*hiroz_MessageCallback)(uintptr_t user_data, const uint8_t *data, uintptr_t len);

/**
 * Called once after the subscriber's last callback reference is released.
 */
typedef void (*hiroz_SubscriberDropCallback)(uintptr_t user_data);

/**
 * CDR encapsulation header for little-endian encoding.
 */
#define hiroz_CDR_HEADER_LE { 0, 1, 0, 0, }



#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

extern void free(void *ptr);

/**
 * Legacy constructor: foreign callback data remains caller-owned. Destroy waits
 * for active events, so an event callback must not destroy its own client. Use
 * `hiroz_action_client_create_owned` for callback-safe, deferred data release.
 */
struct hiroz_action_client_t *hiroz_action_client_create_with_options(struct hiroz_node_t *node,
                                                                      const char *name,
                                                                      const char *action_type,
                                                                      const char *goal_hash,
                                                                      const char *result_hash,
                                                                      const char *cancel_hash,
                                                                      const char *feedback_hash,
                                                                      const char *status_hash,
                                                                      const struct hiroz_CActionQos *qos,
                                                                      hiroz_ActionEventCallback callback,
                                                                      uintptr_t user_data);

/**
 * Create a client taking ownership of its foreign event callback data.
 * `drop_callback` runs exactly once, including on construction failure, after
 * every in-progress delivery and native transport/request clone has released
 * the data. Destroy disables new delivery without waiting for active callbacks,
 * so a callback may destroy its own client. Do not release user_data separately.
 */
struct hiroz_action_client_t *hiroz_action_client_create_owned(struct hiroz_node_t *node,
                                                               const char *name,
                                                               const char *action_type,
                                                               const char *goal_hash,
                                                               const char *result_hash,
                                                               const char *cancel_hash,
                                                               const char *feedback_hash,
                                                               const char *status_hash,
                                                               const struct hiroz_CActionQos *qos,
                                                               hiroz_ActionEventCallback callback,
                                                               uintptr_t user_data,
                                                               hiroz_ActionEventDropCallback drop_callback);

/**
 * Start a request without retaining foreign memory. kind: 0=SendGoal, 1=GetResult, 2=CancelGoal.
 */
struct hiroz_CActionRequest *hiroz_action_request_create(struct hiroz_action_client_t *client,
                                                         int32_t kind,
                                                         const uint8_t *bytes,
                                                         uintptr_t len,
                                                         uint64_t timeout_ms);

/**
 * Returns 1 while pending, 0 with owned response bytes, or a negative error code.
 */
int32_t hiroz_action_request_poll(struct hiroz_CActionRequest *request,
                                  uint8_t **data,
                                  uintptr_t *len);

void hiroz_action_request_destroy(struct hiroz_CActionRequest *request);

bool hiroz_action_client_ready(struct hiroz_action_client_t *client);

/**
 * Disable event delivery and destroy the client handle. Owned clients release
 * foreign data through their drop callback after outstanding references drain.
 * Legacy clients wait for callbacks here and must not call this from an event.
 */
int32_t hiroz_action_client_destroy(struct hiroz_action_client_t *client);

struct hiroz_action_server_t *hiroz_action_server_create_with_options(struct hiroz_node_t *node,
                                                                      const char *name,
                                                                      const char *action_type,
                                                                      const char *goal_hash,
                                                                      const char *result_hash,
                                                                      const char *cancel_hash,
                                                                      const char *feedback_hash,
                                                                      const char *status_hash,
                                                                      const struct hiroz_CActionQos *qos,
                                                                      hiroz_ActionGoalCallback goal_callback,
                                                                      hiroz_ActionExecuteCallback execute_callback,
                                                                      hiroz_ActionCancelCallback cancel_callback,
                                                                      uintptr_t user_data,
                                                                      const uint8_t *default_result,
                                                                      uintptr_t default_result_len,
                                                                      uint64_t result_timeout_ms,
                                                                      uint64_t goal_timeout_ms);

int32_t hiroz_action_server_goal_status(struct hiroz_action_server_t *server, const uint8_t *id);

int32_t hiroz_action_server_execute(struct hiroz_action_server_t *server, const uint8_t *id);

/**
 * Publish a fully encoded FeedbackMessage supplied by the binding.
 */
int32_t hiroz_action_server_publish_feedback_message(struct hiroz_action_server_t *server,
                                                     const uint8_t *bytes,
                                                     uintptr_t len);

int32_t hiroz_action_server_finish(struct hiroz_action_server_t *server,
                                   const uint8_t *id,
                                   const uint8_t *bytes,
                                   uintptr_t len);

/**
 * Read acceptance metadata without exposing a borrowed goal record.
 */
bool hiroz_action_server_goal_info(struct hiroz_action_server_t *server,
                                   const uint8_t *id,
                                   int32_t *sec,
                                   uint32_t *nanosec);

bool hiroz_action_server_feedback_ready(struct hiroz_action_server_t *server);

/**
 * Stop accepting requests without waiting for callbacks. Safe from a callback.
 */
void hiroz_action_server_shutdown(struct hiroz_action_server_t *server);

int32_t hiroz_action_server_destroy(struct hiroz_action_server_t *server);

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
 * Create a service client with endpoint QoS. Null qos uses the defaults.
 */
struct hiroz_service_client_t *hiroz_service_client_create_with_qos(struct hiroz_node_t *node,
                                                                    const char *service_name,
                                                                    const char *req_type_name,
                                                                    const char *req_type_hash,
                                                                    const char *resp_type_name,
                                                                    const char *resp_type_hash,
                                                                    const struct hiroz_qos_profile_t *qos);

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
 * or `timeout_ms` elapses. Returns `Success` (0) if ready, `ServiceTimeout`
 * (-10) on timeout, `NullPointer` (-1) if `client` is null.
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
 * Create a callback service server with endpoint QoS.
 */
struct hiroz_service_server_t *hiroz_service_server_create_with_qos(struct hiroz_node_t *node,
                                                                    const char *service_name,
                                                                    const char *req_type_name,
                                                                    const char *req_type_hash,
                                                                    const char *resp_type_name,
                                                                    const char *resp_type_hash,
                                                                    hiroz_ServiceCallback callback,
                                                                    uintptr_t user_data,
                                                                    const struct hiroz_qos_profile_t *qos);

/**
 * Destroy a service server
 */
int32_t hiroz_service_server_destroy(struct hiroz_service_server_t *server);

/**
 * Start a service request without blocking for a response.
 * Caller owns out_call and must destroy it, including after a successful poll.
 */
int32_t hiroz_service_client_start_call(struct hiroz_service_client_t *client,
                                        const uint8_t *data,
                                        uintptr_t len,
                                        uint64_t timeout_ms,
                                        struct hiroz_service_call_t **out_call);

/**
 * Poll an independent request. Returns 1 while pending, 0 with result, or a negative error.
 * Result bytes are owned by the caller and freed with hiroz_free_bytes.
 */
int32_t hiroz_service_call_poll(struct hiroz_service_call_t *call,
                                uint64_t wait_ms,
                                uint8_t **data,
                                uintptr_t *len);

/**
 * Cancel and release a pending/completed request. Never call concurrently with poll.
 */
int32_t hiroz_service_call_destroy(struct hiroz_service_call_t *call);

/**
 * Create a queued service server, for deferred replies and application worker pools.
 */
struct hiroz_service_server_t *hiroz_service_server_create_queued(struct hiroz_node_t *node,
                                                                  const char *service,
                                                                  const char *type_name,
                                                                  const char *type_hash,
                                                                  const struct hiroz_qos_profile_t *qos);

/**
 * Take one queued request. Returns 1 if no request arrives within wait_ms.
 * The request and its copied bytes are independently caller-owned.
 */
int32_t hiroz_service_server_take_request(struct hiroz_service_server_t *server,
                                          uint64_t wait_ms,
                                          struct hiroz_service_request_t **request,
                                          uint8_t **data,
                                          uintptr_t *len,
                                          struct hiroz_service_request_id_t *id);

/**
 * Reply to a deferred request. Caller must still destroy the request handle.
 */
int32_t hiroz_service_request_reply(struct hiroz_service_request_t *request,
                                    const uint8_t *data,
                                    uintptr_t len);

/**
 * Release an answered or abandoned deferred request.
 */
int32_t hiroz_service_request_destroy(struct hiroz_service_request_t *request);

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
 * Create a subscriber, taking ownership of its foreign callback resources.
 * `drop_callback` runs exactly once, including when creation fails. It runs only
 * after every callback that was already in progress has returned. Destroying
 * the subscriber disables new delivery but does not wait for a running callback,
 * allowing a callback to destroy its own subscriber.
 *
 * # Safety
 * Arguments have the same requirements as `hiroz_subscriber_create_with_qos`.
 * `user_data` must remain valid until `drop_callback` is called, which may occur
 * on any thread. The caller must not release it separately after calling this.
 */
struct hiroz_subscriber_t *hiroz_subscriber_create_owned(struct hiroz_node_t *node,
                                                         const char *topic,
                                                         const char *type_name,
                                                         const char *type_hash,
                                                         hiroz_MessageCallback callback,
                                                         uintptr_t user_data,
                                                         const struct hiroz_qos_profile_t *qos,
                                                         hiroz_SubscriberDropCallback drop_callback);

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
