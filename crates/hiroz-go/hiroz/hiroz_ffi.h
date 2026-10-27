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
 * Opaque publisher handle for FFI
 */
typedef struct hiroz_publisher_t hiroz_publisher_t;

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
 * Opaque service client handle for FFI
 */
typedef struct hiroz_service_client_t {
  struct hiroz_RawServiceClient *inner;
} hiroz_service_client_t;

/**
 * Callback type for service requests.
 * Called with (user_data, request bytes, request len, out response bytes, out response len).
 * Must return 0 on success, non-zero on error.
 *
 * The callback runs on the server's background thread and must not unwind.
 * `request_data` is readable for `request_len` bytes only during the callback;
 * neither it nor the response output pointers may be retained. The output
 * pointers initially contain null and zero. To send a response, set them to a
 * non-null, C `free`-compatible allocation and its nonzero initialized length,
 * no larger than `isize::MAX`, and return zero. This transfers ownership to the
 * server, which reads and frees the allocation after the callback returns.
 * The callback must not access or free it after returning. On any other result,
 * the server neither reads nor frees a response allocation; the callback remains
 * responsible for it. Empty responses are not sent by this callback interface.
 *
 * Any state accessed through `user_data` must remain valid until server
 * destruction returns, and access shared with other threads must be synchronized.
 * Do not destroy the server from its callback: destruction joins this thread.
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
 * Publisher transport options. shm_mode: 0=inherit, 1=disabled, 2=private pool.
 */
typedef struct hiroz_CPublisherOptions {
  const char *encoding;
  int32_t shm_mode;
  uintptr_t shm_pool_bytes;
  uintptr_t shm_threshold;
} hiroz_CPublisherOptions;

/**
 * Sample delivery includes the raw payload/attachment and JSON metadata. All
 * pointers are borrowed until the callback returns; copy before retaining them.
 */
typedef void (*hiroz_SampleCallback)(uintptr_t,
                                     const uint8_t*,
                                     uintptr_t,
                                     const uint8_t*,
                                     uintptr_t,
                                     const char*);

/**
 * CDR encapsulation header for little-endian encoding.
 */
#define hiroz_CDR_HEADER_LE { 0, 1, 0, 0, }

/**
 * CDR encapsulation header for big-endian encoding.
 */
#define hiroz_CDR_HEADER_BE { 0, 0, 0, 0, }



#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

extern void free(void *ptr);

/**
 * Legacy constructor: foreign callback data remains caller-owned. Destroy waits
 * for active events, so an event callback must not destroy its own client. Use
 * `hiroz_action_client_create_owned` for callback-safe, deferred data release.
 *
 * # Safety
 * `node` must be null or a live Hiroz node handle that is not destroyed during
 * this call. Non-null string pointers must reference valid, immutable,
 * NUL-terminated C strings for the call. `qos` must be null or point to an
 * aligned, initialized `CActionQos` that remains readable and unchanged for the
 * call. These arguments are not retained.
 *
 * `callback` and any state identified by `user_data` must remain valid until
 * this call fails or `hiroz_action_client_destroy` returns. Events may arrive
 * on native threads before construction returns; the callback must synchronize
 * access to shared foreign state, must not unwind across the FFI boundary, and
 * must not mutate its borrowed byte buffer or retain it beyond the invocation.
 * The callback must not destroy its own client or wait for its destruction.
 * The caller retains ownership of `user_data` and must destroy a non-null
 * returned handle exactly once.
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
 *
 * # Safety
 * `node` must be null or a live Hiroz node handle that is not destroyed during
 * this call. Non-null string pointers must reference valid, immutable,
 * NUL-terminated C strings for the call. `qos` must be null or point to an
 * aligned, initialized `CActionQos` that remains readable and unchanged for the
 * call. These arguments are not retained.
 *
 * Ownership of the state identified by `user_data` transfers to
 * `drop_callback`, even if construction fails. The state must remain valid
 * until that callback releases it, and both callback functions must remain
 * callable until it finishes. The caller must not release the state separately.
 * Events may begin before construction returns and may run concurrently on
 * native threads. The callbacks must synchronize foreign state and must not
 * unwind across the FFI boundary.
 * Event bytes are immutable and borrowed only for the invocation. The drop
 * callback must be safe to run on whichever thread releases the last reference.
 * A non-null returned handle must be destroyed exactly once. An event may
 * destroy its own client but must not wait for the drop callback or teardown.
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
 *
 * # Safety
 * `client` must be null or a live handle returned by an action client
 * constructor, and must not be destroyed during this call. If non-null,
 * `bytes` must reference `len` initialized, immutable bytes within one live
 * allocation for the call, with `len <= isize::MAX` (including when `len` is
 * zero). The bytes are copied, and the request retains its own transport
 * reference, so neither input needs to outlive this call. A non-null returned
 * request must be destroyed exactly once with `hiroz_action_request_destroy`.
 */
struct hiroz_CActionRequest *hiroz_action_request_create(struct hiroz_action_client_t *client,
                                                         int32_t kind,
                                                         const uint8_t *bytes,
                                                         uintptr_t len,
                                                         uint64_t timeout_ms);

/**
 * Returns 1 while pending, 0 with owned response bytes, or a negative error code.
 *
 * # Safety
 * `request` must be null or a live handle returned by
 * `hiroz_action_request_create`. All polling and destruction of the same
 * request must be serialized. Non-null `data` and `len` must point to aligned,
 * writable storage of their declared types, exclusively accessible for the
 * call and not overlapping each other or the request. On success, the caller
 * owns the returned buffer and must pass its exact pointer and length once to
 * `hiroz_free_bytes`; it must not use C `free` or access the buffer after release.
 * Output storage is unchanged while pending or on error.
 */
int32_t hiroz_action_request_poll(struct hiroz_CActionRequest *request,
                                  uint8_t **data,
                                  uintptr_t *len);

/**
 * Cancel an outstanding request, join its worker, and release its handle.
 *
 * # Safety
 * `request` must be null or a live handle returned by
 * `hiroz_action_request_create`. A non-null handle must be destroyed exactly
 * once, with no concurrent poll, destruction, or later use of that handle.
 * Buffers previously returned by polling remain separately caller-owned.
 */
void hiroz_action_request_destroy(struct hiroz_CActionRequest *request);

/**
 * Check whether all three action services are available.
 *
 * # Safety
 * `client` must be null or a live handle returned by an action client
 * constructor. The handle must remain alive throughout the call; destruction
 * must not run concurrently. Other shared client operations may run concurrently.
 */
bool hiroz_action_client_ready(struct hiroz_action_client_t *client);

/**
 * Disable event delivery and destroy the client handle. Owned clients release
 * foreign data through their drop callback after outstanding references drain.
 * Legacy clients wait for callbacks here and must not call this from an event.
 *
 * # Safety
 * `client` must be null or a live handle returned by an action client
 * constructor. A non-null handle must be destroyed exactly once, with no
 * concurrent use of the handle and no later use, including from callbacks.
 * Existing requests retain their own transport references and may outlive it.
 * For a legacy client, callback code and `user_data` must remain valid until
 * this function returns, and this function must not be called from its event
 * callback or a thread that the callback is waiting for. For an owned client,
 * callback code must remain valid until its drop callback finishes, and
 * `user_data` must remain valid until that callback releases it. An event may
 * destroy an owned client but must not wait for its drop callback or teardown.
 */
int32_t hiroz_action_client_destroy(struct hiroz_action_client_t *client);

/**
 * Create an action server with foreign goal, execution, and cancellation callbacks.
 *
 * # Safety
 * `node` must be null or a live Hiroz node handle that is not destroyed during
 * this call. Non-null string pointers must reference valid, immutable,
 * NUL-terminated C strings for the call. `qos` must be null or point to an
 * aligned, initialized `CActionQos` that remains readable and unchanged for the
 * call. If non-null, `default_result` must reference `default_result_len`
 * initialized, immutable bytes within one live allocation for the call, with
 * `default_result_len <= isize::MAX`. These inputs are not retained.
 *
 * Callback code and the caller-owned state identified by `user_data` must
 * remain valid until construction fails or `hiroz_action_server_destroy`
 * returns. Callbacks may start before construction returns and execute
 * concurrently on native threads; they must synchronize foreign state and
 * must not unwind across the FFI boundary. Callback goal IDs reference 16
 * immutable bytes, and goal payloads are immutable buffers of the supplied
 * length; these pointers are borrowed only for the invocation.
 *
 * The execution callback's writable output pointers are borrowed only for the
 * invocation. Any non-null result it returns, even on error, must transfer
 * ownership of an allocation compatible with C `free`, containing the stated
 * number of initialized bytes within one allocation, with length at most
 * `isize::MAX`. Rust copies and frees that allocation after the callback
 * returns; foreign code must neither access nor free it thereafter.
 * A non-null server handle must be destroyed exactly once. Callbacks must not
 * destroy their own server or wait for its destruction; they may request
 * shutdown with `hiroz_action_server_shutdown` instead.
 */
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

/**
 * Read the current status of a goal, or zero when it is unknown.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor, with no concurrent destruction. If non-null, `id` must reference
 * 16 initialized, immutable bytes within one live allocation for the call.
 * Other server operations may run concurrently.
 */
int32_t hiroz_action_server_goal_status(struct hiroz_action_server_t *server, const uint8_t *id);

/**
 * Transition an accepted goal to executing.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor, with no concurrent destruction. If non-null, `id` must reference
 * 16 initialized, immutable bytes within one live allocation for the call.
 * Other server operations may run concurrently.
 */
int32_t hiroz_action_server_execute(struct hiroz_action_server_t *server, const uint8_t *id);

/**
 * Publish a fully encoded FeedbackMessage supplied by the binding.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor, with no concurrent destruction. If non-null, `bytes` must
 * reference `len` initialized, immutable bytes within one live allocation for
 * the call, with `len <= isize::MAX`. The buffer is not retained. Other server
 * operations may run concurrently.
 */
int32_t hiroz_action_server_publish_feedback_message(struct hiroz_action_server_t *server,
                                                     const uint8_t *bytes,
                                                     uintptr_t len);

/**
 * Complete a goal with a fully encoded result response.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor, with no concurrent destruction. If non-null, `id` must reference
 * 16 initialized, immutable bytes, and `bytes` must reference `len` initialized,
 * immutable bytes, each within one live allocation for the call, with
 * `len <= isize::MAX`. The result is copied; neither buffer is retained. Other
 * server operations may run concurrently.
 */
int32_t hiroz_action_server_finish(struct hiroz_action_server_t *server,
                                   const uint8_t *id,
                                   const uint8_t *bytes,
                                   uintptr_t len);

/**
 * Read acceptance metadata without exposing a borrowed goal record.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor, with no concurrent destruction. If non-null, `id` must reference
 * 16 initialized, immutable bytes within one live allocation for the call.
 * Non-null `sec` and `nanosec` must point to aligned, writable storage of their
 * declared types, exclusively accessible for the call and not overlapping
 * each other, `id`, or the server. Other server operations may run concurrently
 * with separate output storage. Outputs are unchanged when this returns false.
 */
bool hiroz_action_server_goal_info(struct hiroz_action_server_t *server,
                                   const uint8_t *id,
                                   int32_t *sec,
                                   uint32_t *nanosec);

/**
 * Check whether a feedback subscriber is matched.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor. The handle must remain alive throughout the call, with no
 * concurrent destruction. Other server operations may run concurrently.
 */
bool hiroz_action_server_feedback_ready(struct hiroz_action_server_t *server);

/**
 * Stop accepting requests without waiting for callbacks. Safe from a callback.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor. The handle must remain alive throughout the call, with no
 * concurrent destruction. This may run alongside other server operations,
 * including callbacks. It does not release the handle or callback state;
 * those must remain valid until `hiroz_action_server_destroy` completes.
 */
void hiroz_action_server_shutdown(struct hiroz_action_server_t *server);

/**
 * Shut down the server, drain its callbacks, and release its handle.
 *
 * # Safety
 * `server` must be null or a live handle returned by the action server
 * constructor. A non-null handle must be destroyed exactly once, with no
 * concurrent or later use of that handle, including from callbacks. Callback
 * code and `user_data` must remain valid until this function returns. This
 * function must not run inside one of this server's callbacks or on a thread
 * that a callback is waiting for, because it joins all callback workers.
 */
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
 * Validate a ROS TypeDescription and return normalized description and RIHS hash.
 * # Safety
 * description is a valid C string; free result with hiroz_json_destroy.
 */
char *hiroz_schema_validate(const char *description);

/**
 * Register a schema in the node's type description service and process registry.
 * Returns an error if the node was built without that service.
 * # Safety
 * Node and description must be valid.
 */
int32_t hiroz_node_register_schema(struct hiroz_node_t *node, const char *description);

/**
 * Discover topic or service schemas. Empty request_type selects a topic;
 * otherwise request_type and response_type are ROS schema names.
 * Timeout covers the entire discovery, including all type description calls.
 * # Safety
 * Node and C strings must remain live. Free result with hiroz_json_destroy.
 */
char *hiroz_node_discover_schema(struct hiroz_node_t *node,
                                 const char *name,
                                 const char *request_type,
                                 const char *response_type,
                                 uint64_t timeout_ms);

/**
 * Encode a JSON message using the supplied ROS TypeDescription and native CDR codec.
 * # Safety
 * Strings/out pointers must be valid. Free bytes with hiroz_free_bytes.
 */
int32_t hiroz_dynamic_encode(const char *description,
                             const char *values,
                             uint8_t **out,
                             uintptr_t *out_len);

/**
 * Decode CDR to a JSON object with exact integer numbers.
 * # Safety
 * Description and data must be valid; free result with hiroz_json_destroy.
 */
char *hiroz_dynamic_decode(const char *description, const uint8_t *data, uintptr_t len);

/**
 * Return whether schema service registration is available on a node.
 * # Safety
 * Node must be valid or null.
 */
bool hiroz_node_has_type_description_service(struct hiroz_node_t *node);

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
 *
 * # Safety
 * Every non-null string pointer must address a NUL-terminated string. The input
 * byte pointer must be readable for its supplied length, within one allocation,
 * for this call. Non-null output pointers must be aligned, writable, disjoint
 * storage. Inputs are borrowed only until return. On success, ownership of the
 * output allocation transfers to the caller; free it exactly once with
 * `hiroz_free_bytes` and the returned length. Outputs are unchanged on failure.
 */
int32_t hiroz_serialize(const char *type_name,
                        const uint8_t *msg_data,
                        uintptr_t msg_len,
                        uint8_t **out_ptr,
                        uintptr_t *out_len);

/**
 * Deserialize CDR bytes to raw format for Go
 *
 * # Safety
 * Every non-null string pointer must address a NUL-terminated string. The input
 * byte pointer must be readable for its supplied length, within one allocation,
 * for this call. Non-null output pointers must be aligned, writable, disjoint
 * storage. Inputs are borrowed only until return. On success, ownership of the
 * output allocation transfers to the caller; free it exactly once with
 * `hiroz_free_bytes` and the returned length. Outputs are unchanged on failure.
 */
int32_t hiroz_deserialize(const char *type_name,
                          const uint8_t *cdr_data,
                          uintptr_t cdr_len,
                          uint8_t **out_ptr,
                          uintptr_t *out_len);

/**
 * Free bytes allocated by serialize/deserialize.
 *
 * # Safety
 * A non-null pointer with a nonzero length must be an allocation returned by a
 * Hiroz FFI function that documents `hiroz_free_bytes` as its deallocator. Pass
 * the exact returned length, and call this only once after all readers have
 * finished. No other operation may access the allocation during or after this
 * call. A null pointer or zero length is a no-op.
 */
void hiroz_free_bytes(uint8_t *ptr, uintptr_t len);

/**
 * Create a service client
 *
 * # Safety
 * The requirements of [`hiroz_service_client_create_with_qos`] apply, with
 * default QoS. The response type pointers are unused and may be null.
 */
struct hiroz_service_client_t *hiroz_service_client_create(struct hiroz_node_t *node,
                                                           const char *service_name,
                                                           const char *req_type_name,
                                                           const char *req_type_hash,
                                                           const char *resp_type_name,
                                                           const char *resp_type_hash);

/**
 * Create a service client with endpoint QoS. Null qos uses the defaults.
 *
 * # Safety
 * A non-null `node` must be a live handle returned by a node creation function
 * and must not be destroyed or mutably accessed during this call. The required
 * `service_name`, `req_type_name`, and `req_type_hash` strings must be readable,
 * NUL-terminated, and unchanged for this call; null or non-UTF-8 strings cause
 * creation to fail. The response type pointers are unused and may be null.
 * Non-null `qos` must point to an aligned, initialized `CQosProfile` that remains
 * readable and unchanged for this call. These inputs are not retained.
 * The caller owns a non-null result and must release it exactly once with
 * [`hiroz_service_client_destroy`]. A null `node` causes creation to fail.
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
 *
 * # Safety
 * Non-null `client_handle` must be a live service client handle and must not be
 * destroyed or mutably accessed until this call returns. `request_data` must
 * describe `request_len` initialized bytes in one allocation, with a length no
 * larger than `isize::MAX`, readable and unchanged for the call. It must be
 * non-null even for an empty request. Non-null `response_data` and `response_len`
 * must point to aligned, exclusively writable storage for their respective
 * types, disjoint from each other and all inputs, for the call's duration.
 * Null arguments are rejected. On success the caller owns the returned bytes
 * and must release them once using `hiroz_free_bytes` with the exact returned
 * length. On failure the output values are unchanged.
 */
int32_t hiroz_service_client_call(struct hiroz_service_client_t *client_handle,
                                  const uint8_t *request_data,
                                  uintptr_t request_len,
                                  uint8_t **response_data,
                                  uintptr_t *response_len,
                                  uint64_t timeout_ms);

/**
 * Destroy a service client
 *
 * # Safety
 * A non-null `client` must be an owned service client handle returned by a
 * client creation function and not previously destroyed. No other operation
 * may access this handle during or after destruction. Independent call handles
 * returned by [`hiroz_service_client_start_call`] may remain alive. Null is
 * rejected without releasing anything.
 */
int32_t hiroz_service_client_destroy(struct hiroz_service_client_t *client);

/**
 * Wait until at least one matching service server is visible in the graph,
 * or `timeout_ms` elapses. Returns `Success` (0) if ready, `ServiceTimeout`
 * (-10) on timeout, `NullPointer` (-1) if `client` is null.
 *
 * # Safety
 * A non-null `client_handle` must be a live service client handle and must not
 * be destroyed or mutably accessed for the entire wait. Null is rejected.
 */
int32_t hiroz_service_client_wait_for_service(struct hiroz_service_client_t *client_handle,
                                              uint64_t timeout_ms);

extern void free(void *ptr);

/**
 * Create a service server.
 * The server spawns a background thread that polls for incoming requests,
 * invokes the callback for each one, and sends the response.
 *
 * # Safety
 * The requirements of [`hiroz_service_server_create_with_qos`] apply, with
 * default QoS, including the callback and `user_data` lifetime requirements.
 * The response type pointers are unused and may be null.
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
 *
 * # Safety
 * A non-null `node` must be a live handle returned by a node creation function
 * and must not be destroyed or mutably accessed during this call. The required
 * `service_name`, `req_type_name`, and `req_type_hash` strings must be readable,
 * NUL-terminated, and unchanged for this call; null or non-UTF-8 strings cause
 * creation to fail. The response type pointers are unused and may be null.
 * `qos` may be null for defaults; otherwise it must point to an aligned,
 * initialized `CQosProfile`, readable and unchanged for this call. The node,
 * strings, and QoS are not retained. A null `node` causes creation to fail.
 *
 * `callback` must be a valid function obeying [`ServiceCallback`]'s memory,
 * ownership, and threading contract. It may run before this function returns.
 * The function and any state it accesses through `user_data` must remain valid
 * until [`hiroz_service_server_destroy`] returns, with shared access properly
 * synchronized. The callback must not destroy this server. The caller owns a
 * non-null result and must destroy it exactly once outside the callback.
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
 *
 * # Safety
 * A non-null `server` must be an owned handle returned by a service server
 * creation function and not previously destroyed. No other FFI operation may
 * access this handle during or after destruction. This function must not be
 * called from the server's callback: it waits for the callback thread to exit.
 * Keep the callback and its `user_data` state valid until this function returns.
 * Previously taken request handles remain independently owned and must still
 * be destroyed. Null is rejected without releasing anything.
 */
int32_t hiroz_service_server_destroy(struct hiroz_service_server_t *server);

/**
 * Start a service request without blocking for a response.
 * The caller owns the handle written to out_call and must destroy it, including
 * after a successful poll.
 *
 * # Safety
 * Non-null `client` must be a live service client handle and must not be
 * destroyed or mutably accessed during this call. `data` must describe `len`
 * initialized bytes in one allocation, with a length no larger than
 * `isize::MAX`, readable and unchanged for this call. It must be non-null even
 * when `len` is zero. Non-null `out_call` must point to aligned, exclusively
 * writable pointer storage, disjoint from the inputs, for this call. Null
 * arguments are rejected. On success, destroy the returned handle exactly once
 * with [`hiroz_service_call_destroy`]. The request bytes are copied and the
 * returned handle does not borrow the client. Once arguments pass null checks,
 * `out_call` is set to null on failure.
 */
int32_t hiroz_service_client_start_call(struct hiroz_service_client_t *client,
                                        const uint8_t *data,
                                        uintptr_t len,
                                        uint64_t timeout_ms,
                                        struct hiroz_service_call_t **out_call);

/**
 * Poll an independent request. Returns 1 while pending, 0 with result, or a negative error.
 * Result bytes are owned by the caller and freed with hiroz_free_bytes.
 *
 * # Safety
 * Non-null `call` must be a live handle returned by
 * [`hiroz_service_client_start_call`] and must not be destroyed or mutably
 * accessed during this call. Non-null `data` and `len` must point to aligned,
 * exclusively writable storage for their respective types, disjoint from each
 * other and the handle, for the entire poll. Null arguments are rejected.
 * After null checks, the outputs are cleared before polling. On success, the
 * caller must release the returned bytes once with `hiroz_free_bytes`, using
 * the exact returned length. Polling does not release the call handle; it must
 * still be destroyed separately.
 */
int32_t hiroz_service_call_poll(struct hiroz_service_call_t *call,
                                uint64_t wait_ms,
                                uint8_t **data,
                                uintptr_t *len);

/**
 * Cancel and release a pending/completed request. Never call concurrently with poll.
 *
 * # Safety
 * A non-null `call` must be an owned handle returned by
 * [`hiroz_service_client_start_call`] and not previously destroyed. No other
 * operation may access it during or after destruction. Null is rejected without
 * releasing anything. Previously returned result bytes remain caller-owned.
 */
int32_t hiroz_service_call_destroy(struct hiroz_service_call_t *call);

/**
 * Create a queued service server, for deferred replies and application worker pools.
 *
 * # Safety
 * A non-null `node` must be a live handle returned by a node creation function
 * and must not be destroyed or mutably accessed during this call. The required
 * `service`, `type_name`, and `type_hash` strings must be readable,
 * NUL-terminated, and unchanged for this call; null or non-UTF-8 strings cause
 * creation to fail. `qos` may be null for defaults; otherwise it must point to
 * an aligned, initialized `CQosProfile`, readable and unchanged for this call.
 * These inputs are not retained. The caller owns a non-null result and must
 * release it exactly once with [`hiroz_service_server_destroy`]. A null `node`
 * causes creation to fail.
 */
struct hiroz_service_server_t *hiroz_service_server_create_queued(struct hiroz_node_t *node,
                                                                  const char *service,
                                                                  const char *type_name,
                                                                  const char *type_hash,
                                                                  const struct hiroz_qos_profile_t *qos);

/**
 * Take one queued request. Returns 1 if no request arrives within wait_ms.
 * The request and its copied bytes are independently caller-owned.
 *
 * # Safety
 * Non-null `server` must be a live service server handle and must not be
 * destroyed or mutably accessed during the entire wait. Only queued servers
 * support this operation; callback servers are rejected. Every non-null output
 * pointer must address aligned, exclusively writable storage for its declared
 * type, disjoint from the other outputs and the server, throughout this call.
 * Null arguments are rejected. After null checks, `request`, `data`, and `len`
 * are cleared; `id` is written only on success. On success the caller must
 * release the request exactly once with [`hiroz_service_request_destroy`] and
 * the copied bytes once with `hiroz_free_bytes`, using the exact returned
 * length. Neither allocation borrows the other or requires the server handle
 * to remain alive.
 */
int32_t hiroz_service_server_take_request(struct hiroz_service_server_t *server,
                                          uint64_t wait_ms,
                                          struct hiroz_service_request_t **request,
                                          uint8_t **data,
                                          uintptr_t *len,
                                          struct hiroz_service_request_id_t *id);

/**
 * Reply to a deferred request. Caller must still destroy the request handle.
 *
 * # Safety
 * Non-null `request` must be a live handle returned by
 * [`hiroz_service_server_take_request`] and must not be destroyed or mutably
 * accessed during this call. `data` must describe `len` initialized bytes in
 * one allocation, with a length no larger than `isize::MAX`, readable and
 * unchanged for this call. It must be non-null even when `len` is zero. Null
 * arguments are rejected. The bytes are copied; ownership of the input buffer
 * and request handle remains with the caller, including on failure.
 */
int32_t hiroz_service_request_reply(struct hiroz_service_request_t *request,
                                    const uint8_t *data,
                                    uintptr_t len);

/**
 * Release an answered or abandoned deferred request.
 *
 * # Safety
 * A non-null `request` must be an owned handle returned by
 * [`hiroz_service_server_take_request`] and not previously destroyed. No other
 * operation may access it during or after destruction. Null is rejected without
 * releasing anything. This does not release the separately returned request
 * bytes, which remain caller-owned.
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

/**
 * Free a JSON string returned by the topic, dynamic, or graph inspection API.
 * # Safety
 * `value` must be null or a string returned by one of those functions, once only.
 */
void hiroz_json_destroy(char *value);

/**
 * Inspect endpoints. Empty node name queries by fully qualified topic/service;
 * nonempty node name queries by node (namespace `/` is normalized).
 * # Safety
 * All pointers must be live; name, node_name and namespace are C strings.
 */
char *hiroz_graph_endpoints_json(struct hiroz_context_t *ctx,
                                 int32_t endpoint_kind,
                                 const char *name,
                                 const char *node_name,
                                 const char *namespace_);

/**
 * Action and node discovery. kind: 0=all actions, 1=action clients by node,
 * 2=action servers by node, 3=nodes including enclave.
 * # Safety
 * Pointers must remain valid for the call.
 */
char *hiroz_graph_details_json(struct hiroz_context_t *ctx,
                               int32_t kind,
                               const char *node_name,
                               const char *namespace_);

/**
 * Create a publisher with transport options.
 * # Safety
 * Node and strings must be valid; qos and options may be null.
 */
struct hiroz_publisher_t *hiroz_publisher_create_with_options(struct hiroz_node_t *node,
                                                              const char *topic,
                                                              const char *type_name,
                                                              const char *type_hash,
                                                              const struct hiroz_qos_profile_t *qos,
                                                              const struct hiroz_CPublisherOptions *options);

/**
 * Publish serialized data with optional attachment, encoding, and Zenoh timestamp.
 * A null attachment generates normal ROS sequence/source identity metadata; a
 * nonnull attachment (including an empty one) is preserved exactly.
 * # Safety
 * The publisher, strings, data and optional attachment must be live for the call.
 */
int32_t hiroz_publisher_publish_sample(struct hiroz_publisher_t *publisher,
                                       const uint8_t *data,
                                       uintptr_t len,
                                       const uint8_t *attachment,
                                       uintptr_t attachment_len,
                                       const char *encoding,
                                       const char *timestamp);

/**
 * Inspect publisher metadata and currently discovered subscriptions.
 * # Safety
 * Publisher must be live.
 */
char *hiroz_publisher_info_json(struct hiroz_publisher_t *publisher);

/**
 * Subscribe with sample metadata and locality (0=any, 1=session local, 2=remote).
 * Foreign callback ownership is transferred even when creation fails.
 * # Safety
 * Node, strings and callbacks must be valid; qos may be null.
 */
struct hiroz_subscriber_t *hiroz_subscriber_create_samples(struct hiroz_node_t *node,
                                                           const char *topic,
                                                           const char *type_name,
                                                           const char *type_hash,
                                                           hiroz_SampleCallback callback,
                                                           uintptr_t user_data,
                                                           const struct hiroz_qos_profile_t *qos,
                                                           int32_t locality,
                                                           hiroz_SubscriberDropCallback drop_callback);

/**
 * Inspect subscriber metadata and currently discovered publishers.
 * # Safety
 * Subscriber must be live.
 */
char *hiroz_subscriber_info_json(struct hiroz_subscriber_t *subscriber);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* HIROZ_FFI_H */
