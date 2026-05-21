package rosz

/*
#include <stdlib.h>
#include "ros_z_ffi.h"

extern ros_z_ActionGoalCallback getActionGoalCallback();
extern ros_z_ActionExecuteCallback getActionExecuteCallback();
*/
import "C"
import (
	"context"
	"fmt"
	"runtime"
	"runtime/cgo"
	"sync"
	"sync/atomic"
	"unsafe"
)

// Action represents a ROS 2 action (goal/result/feedback pattern).
// Implementations must provide sub-service hashes that match the compound RIHS01
// values used by rmw_zenoh_cpp for its queryables and subscribers.
type Action interface {
	TypeName() string
	// GetGoal returns the goal message type
	GetGoal() Message
	// GetResult returns the result message type
	GetResult() Message
	// GetFeedback returns the feedback message type
	GetFeedback() Message
	// SendGoalHash returns the compound RIHS01 hash for the SendGoal service.
	SendGoalHash() string
	// GetResultHash returns the compound RIHS01 hash for the GetResult service.
	GetResultHash() string
	// CancelGoalHash returns the compound RIHS01 hash for the CancelGoal service.
	CancelGoalHash() string
	// FeedbackMessageHash returns the compound RIHS01 hash for the FeedbackMessage topic.
	FeedbackMessageHash() string
	// StatusHash returns the compound RIHS01 hash for the GoalStatusArray topic.
	StatusHash() string
}

// CGO opaque types are incomplete and cannot be used as type parameters.
// These thin wrappers make atomic.Pointer usable with CGO handles.
type cGoalHandle struct{ p *C.ros_z_goal_handle_t }
type cActionClientHandle struct{ p *C.ros_z_action_client_t }
type cActionServerHandle struct{ p *C.ros_z_action_server_t }

// GoalStatus represents the status of an action goal
type GoalStatus int8

const (
	GoalStatusUnknown   GoalStatus = 0
	GoalStatusAccepted  GoalStatus = 1
	GoalStatusExecuting GoalStatus = 2
	GoalStatusCanceling GoalStatus = 3
	GoalStatusSucceeded GoalStatus = 4
	GoalStatusCanceled  GoalStatus = 5
	GoalStatusAborted   GoalStatus = 6
)

// String returns the string representation of the goal status
func (s GoalStatus) String() string {
	switch s {
	case GoalStatusUnknown:
		return "UNKNOWN"
	case GoalStatusAccepted:
		return "ACCEPTED"
	case GoalStatusExecuting:
		return "EXECUTING"
	case GoalStatusCanceling:
		return "CANCELING"
	case GoalStatusSucceeded:
		return "SUCCEEDED"
	case GoalStatusCanceled:
		return "CANCELED"
	case GoalStatusAborted:
		return "ABORTED"
	default:
		return fmt.Sprintf("GoalStatus(%d)", s)
	}
}

// IsActive returns true if the goal is in an active state
func (s GoalStatus) IsActive() bool {
	return s == GoalStatusAccepted || s == GoalStatusExecuting || s == GoalStatusCanceling
}

// IsTerminal returns true if the goal is in a terminal state
func (s GoalStatus) IsTerminal() bool {
	return s == GoalStatusSucceeded || s == GoalStatusCanceled || s == GoalStatusAborted
}

// GoalID uniquely identifies an action goal (UUID)
type GoalID [16]byte

// GoalHandle represents a client-side handle to an active goal
type GoalHandle struct {
	handle    atomic.Pointer[cGoalHandle]
	client    *ActionClient
	goalID    GoalID
	status    atomic.Int32 // stores GoalStatus as int32
	closeOnce sync.Once
}

// Status returns the current status of the goal
func (h *GoalHandle) Status() GoalStatus {
	return GoalStatus(h.status.Load())
}

// setStatus atomically updates the goal status
func (h *GoalHandle) setStatus(s GoalStatus) {
	h.status.Store(int32(s))
}

// GoalID returns the goal ID
func (h *GoalHandle) GoalID() GoalID {
	return h.goalID
}

// IsActive returns true if the goal is in an active state
func (h *GoalHandle) IsActive() bool {
	return h.Status().IsActive()
}

// IsTerminal returns true if the goal is in a terminal state
func (h *GoalHandle) IsTerminal() bool {
	return h.Status().IsTerminal()
}

// Cancel requests cancellation of the goal
func (h *GoalHandle) Cancel() error {
	hw := h.handle.Load()
	if hw == nil {
		return fmt.Errorf("goal handle is closed")
	}

	result := C.ros_z_action_client_cancel_goal(hw.p)
	if result != 0 {
		return newRoszError(ErrorCodeActionCancelFailed, fmt.Sprintf("failed to cancel goal with code %d", result))
	}
	h.setStatus(GoalStatusCanceling)
	return nil
}

// GetResult waits for and returns the goal result as raw bytes.
// This is a convenience wrapper around GetResultWithContext with a background context.
func (h *GoalHandle) GetResult() ([]byte, error) {
	return h.GetResultWithContext(context.Background())
}

// GetResultWithContext waits for and returns the goal result as raw bytes.
// The context can be used to set a deadline or cancel the wait.
//
// Cancelling ctx causes this function to return ctx.Err() immediately, but
// the underlying Rust call continues running until the action server responds
// or the action client is closed. Do not rely on context cancellation to abort
// server-side execution.
//
// Close() on the ActionClient blocks until all in-flight GetResultWithContext
// goroutines have returned, preventing use-after-free of the C handle.
func (h *GoalHandle) GetResultWithContext(ctx context.Context) ([]byte, error) {
	hw := h.handle.Load()
	if hw == nil {
		return nil, fmt.Errorf("goal handle is closed")
	}

	type ffiResult struct {
		data []byte
		err  error
	}

	ch := make(chan ffiResult, 1)
	h.client.inflight.Add(1)
	goalHandle := hw.p // capture before goroutine to avoid race with Close
	go func() {
		defer h.client.inflight.Done()

		var resultPtr *C.uint8_t
		var resultLen C.uintptr_t

		result := C.ros_z_action_client_get_result(
			goalHandle,
			&resultPtr,
			&resultLen,
		)

		if result != 0 {
			ch <- ffiResult{nil, newRoszError(ErrorCodeActionResultFailed, fmt.Sprintf("failed to get result with code %d", result))}
			return
		}

		resultBytes := C.GoBytes(unsafe.Pointer(resultPtr), C.int(resultLen))
		C.ros_z_free_bytes((*C.uint8_t)(resultPtr), C.uintptr_t(resultLen))
		ch <- ffiResult{resultBytes, nil}
	}()

	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case r := <-ch:
		if r.err == nil {
			h.setStatus(GoalStatusSucceeded)
		}
		return r.data, r.err
	}
}

// Close destroys the goal handle
func (h *GoalHandle) Close() error {
	var err error
	h.closeOnce.Do(func() {
		hw := h.handle.Swap(nil)
		if hw == nil {
			return
		}
		result := C.ros_z_goal_handle_destroy(hw.p)
		if result != 0 {
			err = fmt.Errorf("goal handle close failed (rc=%d): %w", result, ErrCloseFailed)
		}
	})
	return err
}

// ActionClient sends goals to ROS 2 action servers
type ActionClient struct {
	handle    atomic.Pointer[cActionClientHandle]
	node      *Node
	action    string
	inflight  sync.WaitGroup // tracks goroutines blocked inside GetResultWithContext
	closeOnce sync.Once
}

// ActionServer executes ROS 2 action goals and provides feedback
type ActionServer struct {
	handle    atomic.Pointer[cActionServerHandle]
	node      *Node
	action    string
	closure   *actionClosure
	closeOnce sync.Once
}

// ActionClientBuilder builds an ActionClient
type ActionClientBuilder struct {
	node   *Node
	action string
}

// ActionServerBuilder builds an ActionServer
type ActionServerBuilder struct {
	node   *Node
	action string
}

// CreateActionClient creates a new action client builder
func (n *Node) CreateActionClient(action string) *ActionClientBuilder {
	return &ActionClientBuilder{
		node:   n,
		action: action,
	}
}

// CreateActionServer creates a new action server builder
func (n *Node) CreateActionServer(action string) *ActionServerBuilder {
	return &ActionServerBuilder{
		node:   n,
		action: action,
	}
}

// Build creates the action client
func (b *ActionClientBuilder) Build(action Action) (*ActionClient, error) {
	actionC := C.CString(b.action)
	defer C.free(unsafe.Pointer(actionC))

	actionTypeC := C.CString(action.TypeName())
	defer C.free(unsafe.Pointer(actionTypeC))

	goalTypeC := C.CString(action.GetGoal().TypeName())
	defer C.free(unsafe.Pointer(goalTypeC))

	resultTypeC := C.CString(action.GetResult().TypeName())
	defer C.free(unsafe.Pointer(resultTypeC))

	feedbackTypeC := C.CString(action.GetFeedback().TypeName())
	defer C.free(unsafe.Pointer(feedbackTypeC))

	// Use compound sub-service hashes from the Action interface.
	// These match the RIHS01 hashes rmw_zenoh_cpp uses for its queryables/subscribers.
	goalHashC := C.CString(action.SendGoalHash())
	defer C.free(unsafe.Pointer(goalHashC))
	resultHashC := C.CString(action.GetResultHash())
	defer C.free(unsafe.Pointer(resultHashC))
	feedbackHashC := C.CString(action.FeedbackMessageHash())
	defer C.free(unsafe.Pointer(feedbackHashC))

	handle := C.ros_z_action_client_create(
		b.node.handle,
		actionC,
		actionTypeC,
		goalTypeC, goalHashC,
		resultTypeC, resultHashC,
		feedbackTypeC, feedbackHashC,
	)

	if handle == nil {
		return nil, fmt.Errorf("%w: action client for %s", ErrBuildFailed, b.action)
	}

	client := &ActionClient{
		node:   b.node,
		action: b.action,
	}
	client.handle.Store(&cActionClientHandle{p: handle})
	runtime.SetFinalizer(client, (*ActionClient).Close)

	return client, nil
}

// SendGoal sends a goal to the action server and returns a goal handle
func (c *ActionClient) SendGoal(goal Message) (*GoalHandle, error) {
	hw := c.handle.Load()
	if hw == nil {
		return nil, fmt.Errorf("action client is closed")
	}
	h := hw.p

	goalBytes, err := goal.SerializeCDR()
	if err != nil {
		return nil, fmt.Errorf("failed to serialize goal: %w", err)
	}

	if len(goalBytes) == 0 {
		return nil, fmt.Errorf("empty goal")
	}

	// Pin the goal data to prevent GC relocation during the C call
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&goalBytes[0])

	var goalID [16]C.uint8_t
	var handlePtr *C.ros_z_goal_handle_t

	result := C.ros_z_action_client_send_goal(
		h,
		(*C.uint8_t)(unsafe.Pointer(&goalBytes[0])),
		C.uintptr_t(len(goalBytes)),
		(*[16]C.uint8_t)(unsafe.Pointer(&goalID[0])),
		&handlePtr,
	)

	if result != 0 {
		if ErrorCode(result) == ErrorCodeActionGoalRejected {
			return nil, newRoszError(ErrorCodeActionGoalRejected, "goal was rejected by action server")
		}
		return nil, newRoszError(ErrorCode(result), fmt.Sprintf("send goal failed with code %d", result))
	}

	var id GoalID
	for i := 0; i < 16; i++ {
		id[i] = byte(goalID[i])
	}

	handle := &GoalHandle{
		client: c,
		goalID: id,
	}
	handle.handle.Store(&cGoalHandle{p: handlePtr})
	handle.setStatus(GoalStatusAccepted)
	runtime.SetFinalizer(handle, (*GoalHandle).Close)

	return handle, nil
}

// Close destroys the action client.
// Blocks until all in-flight GetResultWithContext goroutines have returned
// so that the C handle is never freed while a goroutine still holds it.
func (c *ActionClient) Close() error {
	var err error
	c.closeOnce.Do(func() {
		c.inflight.Wait() // drain goroutines before freeing handle
		hw := c.handle.Load()
		if hw == nil {
			return
		}
		c.handle.Store(nil)
		result := C.ros_z_action_client_destroy(hw.p)
		if result != 0 {
			err = fmt.Errorf("action client close failed (rc=%d): %w", result, ErrCloseFailed)
		}
	})
	return err
}

// ServerGoalHandle represents a server-side goal handle for managing goal execution
type ServerGoalHandle struct {
	server *ActionServer
	goalID GoalID
}

// IsCancelRequested returns true if the client has requested cancellation of this goal.
// Poll this in long-running execute callbacks to support cooperative cancellation.
func (h *ServerGoalHandle) IsCancelRequested() bool {
	sw := h.server.handle.Load()
	if sw == nil {
		return false
	}
	srv := sw.p
	goalIDBytes := h.goalID
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&goalIDBytes[0])
	result := C.ros_z_action_server_is_cancel_requested(
		srv,
		(*[16]C.uint8_t)(unsafe.Pointer(&goalIDBytes[0])),
	)
	return result != 0
}

// PublishFeedback publishes feedback for the goal
func (h *ServerGoalHandle) PublishFeedback(feedback Message) error {
	sw := h.server.handle.Load()
	if sw == nil {
		return fmt.Errorf("action server is closed")
	}
	srv := sw.p

	feedbackBytes, err := feedback.SerializeCDR()
	if err != nil {
		return fmt.Errorf("failed to serialize feedback: %w", err)
	}

	if len(feedbackBytes) == 0 {
		return nil
	}

	goalIDBytes := h.goalID

	// Pin Go memory before passing to C
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&feedbackBytes[0])
	pinner.Pin(&goalIDBytes[0])

	result := C.ros_z_action_server_publish_feedback(
		srv,
		(*[16]C.uint8_t)(unsafe.Pointer(&goalIDBytes[0])),
		(*C.uint8_t)(unsafe.Pointer(&feedbackBytes[0])),
		C.uintptr_t(len(feedbackBytes)),
	)

	if result != 0 {
		return newRoszError(ErrorCodeActionFeedbackFailed, fmt.Sprintf("publish feedback failed with code %d", result))
	}

	return nil
}

// Succeed marks the goal as successfully completed with a result
func (h *ServerGoalHandle) Succeed(result Message) error {
	return h.storeResult(result, 0)
}

// Abort marks the goal as aborted with a result
func (h *ServerGoalHandle) Abort(result Message) error {
	return h.storeResult(result, 1)
}

// Canceled marks the goal as canceled with a result
func (h *ServerGoalHandle) Canceled(result Message) error {
	return h.storeResult(result, 2)
}

func (h *ServerGoalHandle) storeResult(result Message, op int) error {
	sw := h.server.handle.Load()
	if sw == nil {
		return fmt.Errorf("action server is closed")
	}
	srv := sw.p

	resultBytes, err := result.SerializeCDR()
	if err != nil {
		return fmt.Errorf("failed to serialize result: %w", err)
	}

	goalIDBytes := h.goalID

	// Pin Go memory before passing to C
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&goalIDBytes[0])

	var dataPtr *C.uint8_t
	var dataLen C.uintptr_t
	if len(resultBytes) > 0 {
		pinner.Pin(&resultBytes[0])
		dataPtr = (*C.uint8_t)(unsafe.Pointer(&resultBytes[0]))
		dataLen = C.uintptr_t(len(resultBytes))
	}

	goalIDPtr := (*[16]C.uint8_t)(unsafe.Pointer(&goalIDBytes[0]))

	var res C.int32_t
	switch op {
	case 0:
		res = C.ros_z_action_server_succeed(srv, goalIDPtr, dataPtr, dataLen)
	case 1:
		res = C.ros_z_action_server_abort(srv, goalIDPtr, dataPtr, dataLen)
	case 2:
		res = C.ros_z_action_server_canceled(srv, goalIDPtr, dataPtr, dataLen)
	}

	if res != 0 {
		return fmt.Errorf("store result failed with code %d", res)
	}

	return nil
}

// actionClosure wraps action server callbacks with pinning for safe C access
type actionClosure struct {
	name            string // action name, for logging
	goalCallback    func([]byte) bool
	executeCallback func(handle *ServerGoalHandle, goalData []byte) ([]byte, error)
	goalHandle      cgo.Handle
	executeHandle   cgo.Handle
	selfHandle      cgo.Handle // passed as userData to C; recover via cgo.Handle(userData).Value()
	pinner          *runtime.Pinner
	server          *ActionServer // set after Build(), before any callbacks fire
}

// newActionClosure creates a pinned action closure
func newActionClosure(
	name string,
	goalCallback func([]byte) bool,
	executeCallback func(handle *ServerGoalHandle, goalData []byte) ([]byte, error),
) *actionClosure {
	ac := &actionClosure{
		name:            name,
		goalCallback:    goalCallback,
		executeCallback: executeCallback,
		goalHandle:      cgo.NewHandle(goalCallback),
		executeHandle:   cgo.NewHandle(executeCallback),
	}
	ac.pinner = &runtime.Pinner{}
	ac.pinner.Pin(ac)
	ac.selfHandle = cgo.NewHandle(ac)
	return ac
}

// drop cleans up the action closure
func (ac *actionClosure) drop() {
	ac.goalHandle.Delete()
	ac.executeHandle.Delete()
	ac.selfHandle.Delete()
	ac.pinner.Unpin()
}

// Build creates the action server with callbacks.
// The execute callback receives a ServerGoalHandle for publishing feedback
// and the raw goal bytes. It must return the serialized result.
// Note: callbacks are invoked on Rust/C threads — avoid long blocking operations.
func (b *ActionServerBuilder) Build(
	action Action,
	goalCallback func([]byte) bool,
	executeCallback func(handle *ServerGoalHandle, goalData []byte) ([]byte, error),
) (*ActionServer, error) {
	actionC := C.CString(b.action)
	defer C.free(unsafe.Pointer(actionC))

	actionTypeC := C.CString(action.TypeName())
	defer C.free(unsafe.Pointer(actionTypeC))

	goalTypeC := C.CString(action.GetGoal().TypeName())
	defer C.free(unsafe.Pointer(goalTypeC))

	resultTypeC := C.CString(action.GetResult().TypeName())
	defer C.free(unsafe.Pointer(resultTypeC))

	feedbackTypeC := C.CString(action.GetFeedback().TypeName())
	defer C.free(unsafe.Pointer(feedbackTypeC))

	// Use compound sub-service hashes from the Action interface.
	// These match the RIHS01 hashes rmw_zenoh_cpp uses for its queryables/subscribers.
	goalHashC := C.CString(action.SendGoalHash())
	defer C.free(unsafe.Pointer(goalHashC))
	resultHashC := C.CString(action.GetResultHash())
	defer C.free(unsafe.Pointer(resultHashC))
	feedbackHashC := C.CString(action.FeedbackMessageHash())
	defer C.free(unsafe.Pointer(feedbackHashC))

	// Create pinned closure with both callbacks
	closure := newActionClosure(b.action, goalCallback, executeCallback)

	// Allocate the server struct and set the back-reference BEFORE the C call so
	// that the execute callback can construct ServerGoalHandle the moment a goal
	// arrives, even if os-scheduling places the first goal callback before Build()
	// returns to Go. The handle field is populated after the C call succeeds.
	// Set back-reference so execute callback can construct ServerGoalHandle.
	// Safe: no goals can arrive until the server is discoverable on the network.
	server := &ActionServer{
		node:    b.node,
		action:  b.action,
		closure: closure,
	}
	closure.server = server

	handle := C.ros_z_action_server_create(
		b.node.handle,
		actionC,
		actionTypeC,
		goalTypeC, goalHashC,
		resultTypeC, resultHashC,
		feedbackTypeC, feedbackHashC,
		C.getActionGoalCallback(),
		C.getActionExecuteCallback(),
		// closure.pinner.Pin(closure) guarantees the struct address is stable,
		// making this uintptr cast safe. Do not remove the Pin call above.
		C.uintptr_t(closure.selfHandle),
	)

	if handle == nil {
		closure.drop()
		return nil, fmt.Errorf("%w: action server for %s", ErrBuildFailed, b.action)
	}

	server.handle.Store(&cActionServerHandle{p: handle})
	runtime.SetFinalizer(server, (*ActionServer).Close)

	return server, nil
}

// Close destroys the action server
func (s *ActionServer) Close() error {
	var err error
	s.closeOnce.Do(func() {
		hw := s.handle.Load()
		if hw == nil {
			return
		}
		s.handle.Store(nil)
		result := C.ros_z_action_server_destroy(hw.p)
		if s.closure != nil {
			s.closure.drop()
			s.closure = nil
		}
		if result != 0 {
			err = fmt.Errorf("action server close failed (rc=%d): %w", result, ErrCloseFailed)
		}
	})
	return err
}

//export goActionGoalCallback
func goActionGoalCallback(userData C.uintptr_t, goalData *C.uint8_t, goalLen C.size_t) (rc C.int32_t) {
	// Cast userData back to actionClosure pointer
	closure := cgo.Handle(userData).Value().(*actionClosure)

	// Copy goal data to Go before entering safeCall.
	goGoalData := C.GoBytes(unsafe.Pointer(goalData), C.int(goalLen))

	logger.Debug("goActionGoalCallback", "action", closure.name, "goal_len", int(goalLen))

	var accepted bool
	err := safeCall(func() error {
		accepted = closure.goalCallback(goGoalData)
		return nil
	})
	if err != nil {
		logger.Error("goal callback panic", "action", closure.name)
		return 0 // reject on panic
	}
	if accepted {
		return 1
	}
	return 0
}

//export goActionExecuteCallback
func goActionExecuteCallback(
	userData C.uintptr_t,
	goalIDPtr *C.uint8_t,
	goalData *C.uint8_t,
	goalLen C.size_t,
	resultData **C.uint8_t,
	resultLen *C.size_t,
) (rc C.int32_t) {
	// Cast userData back to actionClosure pointer
	closure := cgo.Handle(userData).Value().(*actionClosure)

	// Extract goal ID (16 bytes) and copy goal data before entering safeCall.
	goalIDSlice := C.GoBytes(unsafe.Pointer(goalIDPtr), 16)
	var goalID GoalID
	copy(goalID[:], goalIDSlice)
	goGoalData := C.GoBytes(unsafe.Pointer(goalData), C.int(goalLen))

	logger.Debug("goActionExecuteCallback", "action", closure.name, "goal_len", int(goalLen))

	err := safeCall(func() error {
		// Construct ServerGoalHandle for feedback publishing
		goalHandle := &ServerGoalHandle{
			server: closure.server,
			goalID: goalID,
		}

		// Call user execute callback with the goal handle
		goResultData, err := closure.executeCallback(goalHandle, goGoalData)
		if err != nil {
			logger.Error("execute callback error", "action", closure.name, "err", err)
			return err
		}

		if len(goResultData) == 0 {
			logger.Error("execute callback returned empty result", "action", closure.name)
			return fmt.Errorf("empty result")
		}

		// Allocate result in C memory (will be freed by Rust)
		*resultLen = C.size_t(len(goResultData))
		*resultData = (*C.uint8_t)(C.CBytes(goResultData))
		return nil
	})

	if err != nil {
		return -1
	}
	return 0
}
