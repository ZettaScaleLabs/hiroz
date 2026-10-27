package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"
extern hiroz_ActionGoalCallback getActionGoalCallback();
extern hiroz_ActionExecuteCallback getActionExecuteCallback();
extern hiroz_ActionCancelCallback getActionCancelCallback();
extern hiroz_ActionEventCallback getActionEventCallback();
extern hiroz_ActionEventDropCallback getActionEventDropCallback();
*/
import "C"
import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"fmt"
	"runtime"
	"runtime/cgo"
	"sync"
	"sync/atomic"
	"time"
	"unsafe"
)

// Action describes the action's messages and all five compound ROS type hashes.
type Action interface {
	TypeName() string
	GetGoal() Message
	GetResult() Message
	GetFeedback() Message
	SendGoalHash() string
	GetResultHash() string
	CancelGoalHash() string
	FeedbackMessageHash() string
	StatusHash() string
}

// ActionMessageCodec is implemented by generated messages. Nested messages must
// align against the whole envelope, particularly the status byte in GetResult.
type ActionMessageCodec interface {
	Message
	PackToRawAt([]byte, int) ([]byte, int)
	UnpackFromRawAt([]byte, int) (int, error)
}

type cActionClientHandle struct{ p *C.hiroz_action_client_t }
type cActionServerHandle struct{ p *C.hiroz_action_server_t }

type GoalStatus int8

const (
	GoalStatusUnknown GoalStatus = iota
	GoalStatusAccepted
	GoalStatusExecuting
	GoalStatusCanceling
	GoalStatusSucceeded
	GoalStatusCanceled
	GoalStatusAborted
)

func (s GoalStatus) String() string {
	names := [...]string{"UNKNOWN", "ACCEPTED", "EXECUTING", "CANCELING", "SUCCEEDED", "CANCELED", "ABORTED"}
	if s >= 0 && int(s) < len(names) {
		return names[s]
	}
	return fmt.Sprintf("GoalStatus(%d)", s)
}
func (s GoalStatus) IsActive() bool   { return s >= GoalStatusAccepted && s <= GoalStatusCanceling }
func (s GoalStatus) IsTerminal() bool { return s >= GoalStatusSucceeded && s <= GoalStatusAborted }

type GoalID [16]byte

// GoalInfo contains the server's acceptance time, as reported on the ROS wire.
type GoalInfo struct {
	GoalID GoalID
	Stamp  time.Time
}
type CancelReturnCode int8

const (
	CancelAccepted CancelReturnCode = iota
	CancelRejected
	CancelUnknownGoal
	CancelGoalTerminated
)

type CancelResponse struct {
	ReturnCode     CancelReturnCode
	GoalsCanceling []GoalInfo
}

// ActionQoS independently configures the five action endpoints.
type ActionQoS struct{ Goal, Result, Cancel, Feedback, Status QosProfile }

func ActionQoSDefault() ActionQoS {
	return ActionQoS{QosServicesDefault(), QosServicesDefault(), QosServicesDefault(), QosDefault(), QosTransientLocal()}
}
func (q ActionQoS) toC() C.hiroz_CActionQos {
	return C.hiroz_CActionQos{goal: q.Goal.toCQos(), result: q.Result.toCQos(), cancel: q.Cancel.toCQos(), feedback: q.Feedback.toCQos(), status: q.Status.toCQos()}
}

// GoalHandle is a client-side goal. Close stops local waits and releases buffered
// events; it does not cancel remote execution. Use Cancel for that.
type GoalHandle struct {
	client    *ActionClient
	goalID    GoalID
	info      GoalInfo
	status    atomic.Int32
	mu        sync.Mutex
	closed    chan struct{}
	feedback  chan []byte
	statuses  chan GoalStatus
	dropped   atomic.Uint64
	closeOnce sync.Once
}

func (h *GoalHandle) GoalID() GoalID     { return h.goalID }
func (h *GoalHandle) Info() GoalInfo     { h.mu.Lock(); defer h.mu.Unlock(); return h.info }
func (h *GoalHandle) Status() GoalStatus { return GoalStatus(h.status.Load()) }
func (h *GoalHandle) IsActive() bool     { return h.Status().IsActive() }
func (h *GoalHandle) IsTerminal() bool   { return h.Status().IsTerminal() }
func (h *GoalHandle) setStatus(status GoalStatus) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.closed == nil {
		h.status.Store(int32(status))
		return
	}
	select {
	case <-h.closed:
		return
	default:
	}
	previous := h.Status()
	if previous.IsTerminal() || (status < previous && status != GoalStatusUnknown) {
		return
	}
	h.status.Store(int32(status))
	if h.statuses != nil {
		select {
		case h.statuses <- status:
		default:
			select {
			case <-h.statuses:
			default:
			}
			h.statuses <- status
		}
	}
}

// Feedback receives raw standalone CDR feedback for this goal. Its capacity is
// configured with WithFeedbackBuffer; DroppedFeedback counts overflow explicitly.
func (h *GoalHandle) Feedback() <-chan []byte { return h.feedback }

// StatusUpdates coalesces updates when the consumer is slower than the server.
func (h *GoalHandle) StatusUpdates() <-chan GoalStatus { return h.statuses }
func (h *GoalHandle) DroppedFeedback() uint64          { return h.dropped.Load() }
func (h *GoalHandle) Cancel() error {
	response, err := h.CancelWithContext(context.Background())
	if err != nil {
		return err
	}
	selected := false
	for _, info := range response.GoalsCanceling {
		if info.GoalID == h.goalID {
			selected = true
			break
		}
	}
	if response.ReturnCode != CancelAccepted || !selected {
		return fmt.Errorf("cancel rejected (code %d): %w", response.ReturnCode, newHirozError(ErrorCodeActionCancelFailed, "goal was not canceled"))
	}
	return nil
}
func (h *GoalHandle) CancelWithContext(ctx context.Context) (CancelResponse, error) {
	if h.client == nil || h.closed == nil {
		return CancelResponse{}, errors.New("goal handle is closed")
	}
	select {
	case <-h.closed:
		return CancelResponse{}, errors.New("goal handle is closed")
	default:
	}
	return h.client.CancelGoals(ctx, h.goalID, time.Time{})
}
func (h *GoalHandle) GetResult() ([]byte, error) { return h.GetResultWithContext(context.Background()) }
func (h *GoalHandle) GetResultWithContext(ctx context.Context) ([]byte, error) {
	_, data, err := h.GetResultWithStatus(ctx)
	return data, err
}

// GetResultWithStatus preserves the server's terminal outcome, including an
// aborted/canceled result. Unknown includes expired goals and is returned as an error.
// Context cancellation cancels the local transport request, not remote execution.
func (h *GoalHandle) GetResultWithStatus(ctx context.Context) (GoalStatus, []byte, error) {
	if h.client == nil || h.closed == nil {
		return GoalStatusUnknown, nil, errors.New("goal handle is closed")
	}
	request := append(cdrHeader(), h.goalID[:]...)
	response, err := h.client.request(ctx, h.closed, 1, request, h.client.resultTimeout)
	if err != nil {
		return GoalStatusUnknown, nil, err
	}
	if err := checkCDR(response, 5); err != nil {
		return GoalStatusUnknown, nil, err
	}
	status := GoalStatus(response[4])
	h.setStatus(status)
	if status == GoalStatusUnknown {
		return status, nil, errors.New("action goal is unknown or its result has expired")
	}
	if !status.IsTerminal() {
		return status, nil, fmt.Errorf("invalid result status %d", status)
	}
	result := h.client.descriptor.GetResult()
	codec, ok := result.(ActionMessageCodec)
	if !ok {
		return status, nil, errors.New("action result requires offset-aware CDR codec")
	}
	if _, err := codec.UnpackFromRawAt(response[4:], 1); err != nil {
		return status, nil, err
	}
	data, err := result.SerializeCDR()
	return status, data, err
}
func (h *GoalHandle) Close() error {
	h.closeOnce.Do(func() {
		h.mu.Lock()
		if h.closed != nil {
			close(h.closed)
		}
		if h.feedback != nil {
			close(h.feedback)
		}
		if h.statuses != nil {
			close(h.statuses)
		}
		h.mu.Unlock()
		if h.client != nil {
			h.client.goalsMu.Lock()
			delete(h.client.goals, h.goalID)
			h.client.goalsMu.Unlock()
		}
	})
	return nil
}

type ActionClient struct {
	handle                        atomic.Pointer[cActionClientHandle]
	node                          *Node
	action                        string
	descriptor                    Action
	mu                            sync.Mutex
	inflight                      sync.WaitGroup
	done                          chan struct{}
	goalsMu                       sync.Mutex
	goals                         map[GoalID]*GoalHandle
	events                        cgo.Handle
	feedbackBuffer                int
	requestTimeout, resultTimeout time.Duration
	closeOnce                     sync.Once
}
type ActionClientBuilder struct {
	node                          *Node
	action                        string
	qos                           *ActionQoS
	feedbackBuffer                int
	requestTimeout, resultTimeout time.Duration
}
type ActionServerBuilder struct {
	node               *Node
	action             string
	qos                *ActionQoS
	resultTimeout      time.Duration
	goalTimeout        time.Duration
	goalCallbackWithID func(GoalID, []byte) bool
	cancelCallback     func(*ServerGoalHandle) bool
}

func (n *Node) CreateActionClient(action string) *ActionClientBuilder {
	return &ActionClientBuilder{node: n, action: action, feedbackBuffer: 128, requestTimeout: 10 * time.Second}
}
func (n *Node) CreateActionServer(action string) *ActionServerBuilder {
	return &ActionServerBuilder{node: n, action: action, resultTimeout: 15 * time.Minute}
}
func (b *ActionClientBuilder) WithQoS(qos ActionQoS) *ActionClientBuilder { b.qos = &qos; return b }
func (b *ActionServerBuilder) WithQoS(qos ActionQoS) *ActionServerBuilder { b.qos = &qos; return b }
func (b *ActionClientBuilder) WithFeedbackBuffer(capacity int) *ActionClientBuilder {
	b.feedbackBuffer = capacity
	return b
}
func (b *ActionClientBuilder) WithRequestTimeout(timeout time.Duration) *ActionClientBuilder {
	b.requestTimeout = timeout
	return b
}

// WithResultTimeout sets the transport lifetime of a result wait. Zero uses the
// caller's context deadline or a 100-year transport lifetime; all waits remain cancelable.
func (b *ActionClientBuilder) WithResultTimeout(timeout time.Duration) *ActionClientBuilder {
	b.resultTimeout = timeout
	return b
}

// WithResultTimeout controls how long completed results remain retrievable.
func (b *ActionServerBuilder) WithResultTimeout(timeout time.Duration) *ActionServerBuilder {
	b.resultTimeout = timeout
	return b
}

// WithGoalTimeout aborts unfinished goals after the configured lifetime. Zero disables it.
func (b *ActionServerBuilder) WithGoalTimeout(timeout time.Duration) *ActionServerBuilder {
	b.goalTimeout = timeout
	return b
}

// WithGoalCallback receives the client's UUID before acceptance. It replaces the
// payload-only decision passed to Build or BuildDeferred.
func (b *ActionServerBuilder) WithGoalCallback(callback func(GoalID, []byte) bool) *ActionServerBuilder {
	b.goalCallbackWithID = callback
	return b
}
func (b *ActionServerBuilder) WithCancelCallback(callback func(*ServerGoalHandle) bool) *ActionServerBuilder {
	b.cancelCallback = callback
	return b
}

func validateAction(action Action) error {
	if action == nil {
		return errors.New("nil action")
	}
	if _, ok := action.GetResult().(ActionMessageCodec); !ok {
		return errors.New("action result requires PackToRawAt and UnpackFromRawAt; regenerate messages with hiroz-codegen-go")
	}
	return nil
}
func actionStrings(name string, a Action) ([]*C.char, func()) {
	strings := []string{name, a.TypeName(), a.SendGoalHash(), a.GetResultHash(), a.CancelGoalHash(), a.FeedbackMessageHash(), a.StatusHash()}
	ptrs := make([]*C.char, len(strings))
	for i, s := range strings {
		ptrs[i] = C.CString(s)
	}
	return ptrs, func() {
		for _, p := range ptrs {
			C.free(unsafe.Pointer(p))
		}
	}
}
func (b *ActionClientBuilder) Build(action Action) (*ActionClient, error) {
	node, release, err := b.node.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	if err := validateAction(action); err != nil {
		return nil, err
	}
	if b.feedbackBuffer < 1 || b.requestTimeout <= 0 || b.resultTimeout < 0 {
		return nil, errors.New("invalid action buffer or timeout")
	}
	c := &ActionClient{node: b.node, action: b.action, descriptor: action, done: make(chan struct{}), goals: make(map[GoalID]*GoalHandle), feedbackBuffer: b.feedbackBuffer, requestTimeout: b.requestTimeout, resultTimeout: b.resultTimeout}
	c.events = cgo.NewHandle(c)
	strings, free := actionStrings(b.action, action)
	defer free()
	qos := ActionQoSDefault()
	if b.qos != nil {
		qos = *b.qos
	}
	cqos := qos.toC()
	// Native code owns events from this call onward, including construction
	// failure. Its final callback reference releases the handle after delivery.
	handle := C.hiroz_action_client_create_owned(node, strings[0], strings[1], strings[2], strings[3], strings[4], strings[5], strings[6], &cqos, C.getActionEventCallback(), C.uintptr_t(c.events), C.getActionEventDropCallback())
	if handle == nil {
		return nil, fmt.Errorf("%w: action client for %s", ErrBuildFailed, b.action)
	}
	c.handle.Store(&cActionClientHandle{handle})
	runtime.SetFinalizer(c, (*ActionClient).Close)
	return c, nil
}
func (c *ActionClient) request(ctx context.Context, goalClosed <-chan struct{}, kind int, bytes []byte, timeout time.Duration) ([]byte, error) {
	if ctx == nil {
		return nil, errors.New("nil context")
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	c.mu.Lock()
	hw := c.handle.Load()
	if hw == nil {
		c.mu.Unlock()
		return nil, errors.New("action client is closed")
	}
	if err := ctx.Err(); err != nil {
		c.mu.Unlock()
		return nil, err
	}
	select {
	case <-goalClosed:
		c.mu.Unlock()
		return nil, errors.New("goal handle is closed")
	default:
	}
	if deadline, ok := ctx.Deadline(); ok {
		remaining := time.Until(deadline)
		if timeout == 0 || remaining < timeout {
			timeout = remaining
		}
	}
	// Zenoh requires a finite duration; cancellation still releases the query promptly.
	if timeout == 0 {
		timeout = 100 * 365 * 24 * time.Hour
	}
	if timeout <= 0 {
		c.mu.Unlock()
		return nil, context.DeadlineExceeded
	}
	timeoutMillis := timeout / time.Millisecond
	if timeout%time.Millisecond != 0 {
		timeoutMillis++
	}
	c.inflight.Add(1)
	request := C.hiroz_action_request_create(hw.p, C.int32_t(kind), (*C.uint8_t)(unsafe.Pointer(&bytes[0])), C.size_t(len(bytes)), C.uint64_t(timeoutMillis))
	c.mu.Unlock()
	defer c.inflight.Done()
	if request == nil {
		return nil, errors.New("failed to start action request")
	}
	defer C.hiroz_action_request_destroy(request)
	ticker := time.NewTicker(5 * time.Millisecond)
	defer ticker.Stop()
	for {
		var data *C.uint8_t
		var size C.size_t
		rc := C.hiroz_action_request_poll(request, &data, &size)
		if rc == 0 {
			bytes := C.GoBytes(unsafe.Pointer(data), C.int(size))
			C.hiroz_free_bytes(data, size)
			return bytes, nil
		}
		if rc != 1 {
			if err := ctx.Err(); err != nil {
				return nil, err
			}
			return nil, newHirozError(ErrorCode(rc), "action request failed")
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-c.done:
			return nil, errors.New("action client is closed")
		case <-goalClosed:
			return nil, errors.New("goal handle is closed")
		case <-ticker.C:
		}
	}
}
func (c *ActionClient) SendGoal(goal Message) (*GoalHandle, error) {
	return c.SendGoalWithContext(context.Background(), goal)
}
func (c *ActionClient) SendGoalWithContext(ctx context.Context, goal Message) (*GoalHandle, error) {
	if c.descriptor == nil {
		return nil, errors.New("action client is closed")
	}
	bytes, err := goal.SerializeCDR()
	if err != nil {
		return nil, err
	}
	if err = checkCDR(bytes, 4); err != nil {
		return nil, err
	}
	var id GoalID
	if _, err = rand.Read(id[:]); err != nil {
		return nil, err
	}
	id[6] = (id[6] & 0x0f) | 0x40
	id[8] = (id[8] & 0x3f) | 0x80
	h := &GoalHandle{client: c, goalID: id, info: GoalInfo{GoalID: id}, closed: make(chan struct{}), feedback: make(chan []byte, c.feedbackBuffer), statuses: make(chan GoalStatus, 1)}
	c.goalsMu.Lock()
	c.goals[id] = h
	c.goalsMu.Unlock()
	request := append(cdrHeader(), id[:]...)
	request = append(request, bytes[4:]...)
	response, err := c.request(ctx, h.closed, 0, request, c.requestTimeout)
	if err != nil {
		h.Close()
		return nil, err
	}
	if err = checkCDR(response, 16); err != nil {
		h.Close()
		return nil, err
	}
	if response[4] == 0 {
		h.Close()
		return nil, newHirozError(ErrorCodeActionGoalRejected, "goal was rejected by action server")
	}
	h.mu.Lock()
	h.info.Stamp = wireTime(response[8:16])
	h.mu.Unlock()
	h.setStatus(GoalStatusAccepted)
	runtime.SetFinalizer(h, (*GoalHandle).Close)
	return h, nil
}

// CancelGoals implements the ROS cancellation selection: zero ID and zero time
// selects all; ID with zero time selects only that ID; a timestamp adds every goal
// accepted at or before that time. The response distinguishes rejected/unknown/finished.
func (c *ActionClient) CancelGoals(ctx context.Context, id GoalID, before time.Time) (CancelResponse, error) {
	request := append(cdrHeader(), id[:]...)
	stamp, err := encodeTime(before)
	if err != nil {
		return CancelResponse{}, err
	}
	request = append(request, stamp...)
	response, err := c.request(ctx, nil, 2, request, c.requestTimeout)
	if err != nil {
		return CancelResponse{}, err
	}
	if err = checkCDR(response, 12); err != nil {
		return CancelResponse{}, err
	}
	result := CancelResponse{ReturnCode: CancelReturnCode(response[4])}
	count := uint64(binary.LittleEndian.Uint32(response[8:12]))
	if count > uint64((len(response)-12)/24) {
		return result, errors.New("malformed cancellation response")
	}
	for i := 0; i < int(count); i++ {
		pos := 12 + 24*i
		var id GoalID
		copy(id[:], response[pos:pos+16])
		result.GoalsCanceling = append(result.GoalsCanceling, GoalInfo{id, wireTime(response[pos+16 : pos+24])})
		c.goalsMu.Lock()
		h := c.goals[id]
		c.goalsMu.Unlock()
		if h != nil {
			h.setStatus(GoalStatusCanceling)
		}
	}
	return result, nil
}
func (c *ActionClient) CancelAllGoals(ctx context.Context) (CancelResponse, error) {
	return c.CancelGoals(ctx, GoalID{}, time.Time{})
}
func (c *ActionClient) CancelGoalsBefore(ctx context.Context, before time.Time) (CancelResponse, error) {
	return c.CancelGoals(ctx, GoalID{}, before)
}
func (c *ActionClient) IsServerAvailable() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	h := c.handle.Load()
	return h != nil && bool(C.hiroz_action_client_ready(h.p))
}
func (c *ActionClient) WaitForServer(ctx context.Context) error {
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	for {
		if c.IsServerAvailable() {
			return nil
		}
		if c.handle.Load() == nil {
			return errors.New("action client is closed")
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-c.done:
			return errors.New("action client is closed")
		case <-ticker.C:
		}
	}
}
func (c *ActionClient) Close() error {
	c.closeOnce.Do(func() {
		c.mu.Lock()
		h := c.handle.Swap(nil)
		if c.done != nil {
			close(c.done)
		}
		c.mu.Unlock()
		c.inflight.Wait()
		if h != nil {
			C.hiroz_action_client_destroy(h.p)
		}
		c.goalsMu.Lock()
		goals := make([]*GoalHandle, 0, len(c.goals))
		for _, g := range c.goals {
			goals = append(goals, g)
		}
		c.goalsMu.Unlock()
		for _, g := range goals {
			g.Close()
		}
	})
	return nil
}

//export goActionEventDropCallback
func goActionEventDropCallback(userData C.uintptr_t) {
	cgo.Handle(userData).Delete()
}

//export goActionEventCallback
func goActionEventCallback(userData C.uintptr_t, kind C.int32_t, data *C.uint8_t, size C.size_t) {
	c := cgo.Handle(userData).Value().(*ActionClient)
	bytes := C.GoBytes(unsafe.Pointer(data), C.int(size))
	_ = safeCall(func() error {
		if kind == 0 {
			if err := checkCDR(bytes, 20); err != nil {
				return err
			}
			var id GoalID
			copy(id[:], bytes[4:20])
			c.goalsMu.Lock()
			h := c.goals[id]
			c.goalsMu.Unlock()
			if h == nil {
				return nil
			}
			payload := append(cdrHeader(), bytes[20:]...)
			h.mu.Lock()
			defer h.mu.Unlock()
			select {
			case <-h.closed:
				return nil
			default:
			}
			select {
			case h.feedback <- payload:
			default:
				h.dropped.Add(1)
			}
		} else {
			if err := checkCDR(bytes, 8); err != nil {
				return err
			}
			count := uint64(binary.LittleEndian.Uint32(bytes[4:8]))
			offset := 8
			if count > uint64((len(bytes)-8)/25) {
				return errors.New("malformed status array")
			}
			for i := uint64(0); i < count; i++ {

				if offset+25 > len(bytes) {
					return errors.New("truncated status array")
				}
				var id GoalID
				copy(id[:], bytes[offset:offset+16])
				offset += 16
				offset += (4 - (offset-4)%4) % 4
				if offset+9 > len(bytes) {
					return errors.New("truncated status array")
				}
				c.goalsMu.Lock()
				h := c.goals[id]
				c.goalsMu.Unlock()
				if h != nil {
					h.mu.Lock()
					h.info.Stamp = wireTime(bytes[offset : offset+8])
					h.mu.Unlock()
					h.setStatus(GoalStatus(bytes[offset+8]))
				}
				offset += 9
			}
		}
		return nil
	})
}

// ActionServer retains callbacks until asynchronous close cleanup drains them.
// Execute callbacks should observe ServerGoalHandle.Context() for prompt shutdown.
type ActionServer struct {
	handle     atomic.Pointer[cActionServerHandle]
	node       *Node
	action     string
	descriptor Action
	closure    *actionClosure
	mu         sync.RWMutex
	ctx        context.Context
	cancel     context.CancelFunc
	closedDone chan struct{}
	closeOnce  sync.Once
}
type ServerGoalHandle struct {
	server *ActionServer
	goalID GoalID
}

func (h *ServerGoalHandle) GoalID() GoalID { return h.goalID }
func (h *ServerGoalHandle) Context() context.Context {
	if h.server.ctx == nil {
		return context.Background()
	}
	return h.server.ctx
}
func (h *ServerGoalHandle) Status() GoalStatus {
	h.server.mu.RLock()
	defer h.server.mu.RUnlock()
	s := h.server.handle.Load()
	if s == nil {
		return GoalStatusUnknown
	}
	return GoalStatus(C.hiroz_action_server_goal_status(s.p, (*C.uint8_t)(unsafe.Pointer(&h.goalID[0]))))
}
func (h *ServerGoalHandle) IsActive() bool          { return h.Status().IsActive() }
func (h *ServerGoalHandle) IsTerminal() bool        { return h.Status().IsTerminal() }
func (h *ServerGoalHandle) IsCancelRequested() bool { return h.Status() == GoalStatusCanceling }

// Execute transitions a deferred goal from Accepted to Executing. The application
// owns scheduling and must eventually call Succeed, Abort, or Canceled.
func (h *ServerGoalHandle) Execute() error {
	h.server.mu.RLock()
	defer h.server.mu.RUnlock()
	s := h.server.handle.Load()
	if s == nil {
		return errors.New("action server is closed")
	}
	if C.hiroz_action_server_execute(s.p, (*C.uint8_t)(unsafe.Pointer(&h.goalID[0]))) != 0 {
		return errors.New("goal is not accepted")
	}
	return nil
}
func (h *ServerGoalHandle) PublishFeedback(feedback Message) error {
	bytes, err := feedback.SerializeCDR()
	if err != nil {
		return err
	}
	if err = checkCDR(bytes, 4); err != nil {
		return err
	}
	envelope := append(cdrHeader(), h.goalID[:]...)
	envelope = append(envelope, bytes[4:]...)
	h.server.mu.RLock()
	defer h.server.mu.RUnlock()
	s := h.server.handle.Load()
	if s == nil {
		return errors.New("action server is closed")
	}
	if C.hiroz_action_server_publish_feedback_message(s.p, (*C.uint8_t)(unsafe.Pointer(&envelope[0])), C.size_t(len(envelope))) != 0 {
		return errors.New("feedback publish failed")
	}
	return nil
}
func (h *ServerGoalHandle) Succeed(result Message) error {
	return h.storeResult(result, GoalStatusSucceeded)
}
func (h *ServerGoalHandle) Abort(result Message) error {
	return h.storeResult(result, GoalStatusAborted)
}
func (h *ServerGoalHandle) Canceled(result Message) error {
	return h.storeResult(result, GoalStatusCanceled)
}
func (h *ServerGoalHandle) storeResult(result Message, status GoalStatus) error {
	bytes, err := encodeResult(result, status)
	if err != nil {
		return err
	}
	h.server.mu.RLock()
	defer h.server.mu.RUnlock()
	s := h.server.handle.Load()
	if s == nil {
		return errors.New("action server is closed")
	}
	if C.hiroz_action_server_finish(s.p, (*C.uint8_t)(unsafe.Pointer(&h.goalID[0])), (*C.uint8_t)(unsafe.Pointer(&bytes[0])), C.size_t(len(bytes))) != 0 {
		return errors.New("invalid terminal transition or goal already completed")
	}
	return nil
}
func encodeResult(result Message, status GoalStatus) ([]byte, error) {
	codec, ok := result.(ActionMessageCodec)
	if !ok {
		return nil, errors.New("action result requires offset-aware CDR codec")
	}
	bytes := append(cdrHeader(), byte(status))
	bytes, _ = codec.PackToRawAt(bytes, 1)
	return bytes, nil
}
func cdrHeader() []byte { return []byte{0, 1, 0, 0} }
func checkCDR(bytes []byte, min int) error {
	if len(bytes) < min {
		return errors.New("truncated action CDR message")
	}
	if bytes[0] != 0 || bytes[1] != 1 {
		return errors.New("action requires CDR1 little-endian encoding")
	}
	return nil
}
func wireTime(bytes []byte) time.Time {
	sec := int32(binary.LittleEndian.Uint32(bytes[:4]))
	nsec := binary.LittleEndian.Uint32(bytes[4:])
	if sec == 0 && nsec == 0 {
		return time.Time{}
	}
	return time.Unix(int64(sec), int64(nsec))
}
func encodeTime(t time.Time) ([]byte, error) {
	bytes := make([]byte, 8)
	if t.IsZero() {
		return bytes, nil
	}
	sec := t.Unix()
	if sec < -(1<<31) || sec > 1<<31-1 {
		return nil, errors.New("timestamp exceeds ROS int32 seconds")
	}
	binary.LittleEndian.PutUint32(bytes, uint32(sec))
	binary.LittleEndian.PutUint32(bytes[4:], uint32(t.Nanosecond()))
	return bytes, nil
}

type actionClosure struct {
	name               string
	goalCallback       func([]byte) bool
	goalCallbackWithID func(GoalID, []byte) bool
	executeCallback    func(*ServerGoalHandle, []byte) ([]byte, error)
	cancelCallback     func(*ServerGoalHandle) bool
	deferred           bool
	selfHandle         cgo.Handle
	server             *ActionServer
	ready              chan struct{}
}

func (b *ActionServerBuilder) Build(action Action, goalCallback func([]byte) bool, executeCallback func(*ServerGoalHandle, []byte) ([]byte, error)) (*ActionServer, error) {
	return b.build(action, goalCallback, executeCallback, false)
}

// BuildDeferred calls accepted after acceptance without starting execution. It may
// retain the handle, schedule work, call Execute, and then explicitly complete it.
func (b *ActionServerBuilder) BuildDeferred(action Action, goalCallback func([]byte) bool, accepted func(*ServerGoalHandle, []byte)) (*ActionServer, error) {
	if accepted == nil {
		return nil, errors.New("nil accepted callback")
	}
	return b.build(action, goalCallback, func(h *ServerGoalHandle, data []byte) ([]byte, error) { accepted(h, data); return nil, nil }, true)
}
func (b *ActionServerBuilder) build(action Action, goalCallback func([]byte) bool, executeCallback func(*ServerGoalHandle, []byte) ([]byte, error), deferred bool) (*ActionServer, error) {
	node, release, err := b.node.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	if err := validateAction(action); err != nil {
		return nil, err
	}
	if (goalCallback == nil && b.goalCallbackWithID == nil) || executeCallback == nil {
		return nil, errors.New("nil action callback")
	}
	if b.resultTimeout < 0 || b.goalTimeout < 0 {
		return nil, errors.New("negative result retention")
	}
	defaultResult, err := encodeResult(action.GetResult(), GoalStatusUnknown)
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(context.Background())
	server := &ActionServer{node: b.node, action: b.action, descriptor: action, ctx: ctx, cancel: cancel, closedDone: make(chan struct{})}
	closure := &actionClosure{name: b.action, goalCallback: goalCallback, goalCallbackWithID: b.goalCallbackWithID, executeCallback: executeCallback, cancelCallback: b.cancelCallback, deferred: deferred, server: server, ready: make(chan struct{})}
	closure.selfHandle = cgo.NewHandle(closure)
	server.closure = closure
	strings, free := actionStrings(b.action, action)
	defer free()
	qos := ActionQoSDefault()
	if b.qos != nil {
		qos = *b.qos
	}
	cqos := qos.toC()
	handle := C.hiroz_action_server_create_with_options(node, strings[0], strings[1], strings[2], strings[3], strings[4], strings[5], strings[6], &cqos, C.getActionGoalCallback(), C.getActionExecuteCallback(), C.getActionCancelCallback(), C.uintptr_t(closure.selfHandle), (*C.uint8_t)(unsafe.Pointer(&defaultResult[0])), C.size_t(len(defaultResult)), C.uint64_t(b.resultTimeout/time.Millisecond), C.uint64_t(b.goalTimeout/time.Millisecond))
	if handle == nil {
		cancel()
		close(closure.ready)
		closure.selfHandle.Delete()
		return nil, fmt.Errorf("%w: action server for %s", ErrBuildFailed, b.action)
	}
	server.handle.Store(&cActionServerHandle{handle})
	close(closure.ready)
	runtime.SetFinalizer(server, (*ActionServer).Close)
	return server, nil
}

// Close stops new work and initiates cleanup. It is safe inside a callback.
// Callback storage and the native session remain retained until every worker
// has returned and every server endpoint has been destroyed, even if the node
// and context are closed before WaitClosed returns.
func (s *ActionServer) Close() error {
	s.closeOnce.Do(func() {
		s.mu.Lock()
		h := s.handle.Swap(nil)
		if s.cancel != nil {
			s.cancel()
		}
		if h != nil {
			C.hiroz_action_server_shutdown(h.p)
		}
		s.mu.Unlock()
		if h != nil {
			go func() {
				C.hiroz_action_server_destroy(h.p)
				s.closure.selfHandle.Delete()
				close(s.closedDone)
			}()
		} else if s.closedDone != nil {
			close(s.closedDone)
		}
	})
	return nil
}

// WaitClosed waits for cleanup after Close, including all active callbacks.
// Call it outside callbacks; a callback cannot wait for its own completion.
func (s *ActionServer) WaitClosed(ctx context.Context) error {
	if s.closedDone == nil {
		return nil
	}
	select {
	case <-s.closedDone:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

//export goActionGoalCallback
func goActionGoalCallback(userData C.uintptr_t, id *C.uint8_t, data *C.uint8_t, size C.size_t) (rc C.int32_t) {
	closure := cgo.Handle(userData).Value().(*actionClosure)
	<-closure.ready
	if closure.server.ctx.Err() != nil {
		return 0
	}
	bytes := C.GoBytes(unsafe.Pointer(data), C.int(size))
	accepted := false
	if safeCall(func() error {
		if closure.goalCallbackWithID != nil {
			var goalID GoalID
			copy(goalID[:], C.GoBytes(unsafe.Pointer(id), 16))
			accepted = closure.goalCallbackWithID(goalID, bytes)
		} else {
			accepted = closure.goalCallback(bytes)
		}
		return nil
	}) != nil || !accepted {
		return 0
	}
	if closure.deferred {
		return 2
	}
	return 1
}

//export goActionCancelCallback
func goActionCancelCallback(userData C.uintptr_t, id *C.uint8_t) (rc C.int32_t) {
	closure := cgo.Handle(userData).Value().(*actionClosure)
	<-closure.ready
	if closure.server.ctx.Err() != nil {
		return 0
	}
	h := &ServerGoalHandle{server: closure.server}
	copy(h.goalID[:], C.GoBytes(unsafe.Pointer(id), 16))
	accepted := true
	if safeCall(func() error {
		if closure.cancelCallback != nil {
			accepted = closure.cancelCallback(h)
		}
		return nil
	}) != nil || !accepted {
		return 0
	}
	return 1
}

//export goActionExecuteCallback
func goActionExecuteCallback(userData C.uintptr_t, id *C.uint8_t, data *C.uint8_t, size C.size_t, result **C.uint8_t, resultSize *C.size_t) (rc C.int32_t) {
	closure := cgo.Handle(userData).Value().(*actionClosure)
	<-closure.ready
	if closure.server.ctx.Err() != nil {
		return -1
	}
	h := &ServerGoalHandle{server: closure.server}
	copy(h.goalID[:], C.GoBytes(unsafe.Pointer(id), 16))
	bytes := C.GoBytes(unsafe.Pointer(data), C.int(size))
	err := safeCall(func() error {
		response, err := closure.executeCallback(h, bytes)
		if err != nil {
			return err
		}
		if h.IsTerminal() || (closure.deferred && len(response) == 0) {
			return nil
		}
		message := closure.server.descriptor.GetResult()
		if err = message.DeserializeCDR(response); err != nil {
			return err
		}
		status := GoalStatusSucceeded
		if h.IsCancelRequested() {
			status = GoalStatusCanceled
		}
		envelope, err := encodeResult(message, status)
		if err != nil {
			return err
		}
		*result = (*C.uint8_t)(C.CBytes(envelope))
		*resultSize = C.size_t(len(envelope))
		return nil
	})
	if err != nil {
		return -1
	}
	return 0
}

// Info returns the acceptance timestamp and UUID while the goal remains retained.
func (h *ServerGoalHandle) Info() (GoalInfo, error) {
	h.server.mu.RLock()
	defer h.server.mu.RUnlock()
	s := h.server.handle.Load()
	if s == nil {
		return GoalInfo{}, errors.New("action server is closed")
	}
	var sec C.int32_t
	var ns C.uint32_t
	if !bool(C.hiroz_action_server_goal_info(s.p, (*C.uint8_t)(unsafe.Pointer(&h.goalID[0])), &sec, &ns)) {
		return GoalInfo{}, errors.New("unknown goal")
	}
	return GoalInfo{h.goalID, time.Unix(int64(sec), int64(ns))}, nil
}

// WaitForFeedbackSubscriber waits for a transport match before sending feedback.
func (s *ActionServer) WaitForFeedbackSubscriber(ctx context.Context) error {
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	for {
		s.mu.RLock()
		h := s.handle.Load()
		ready := h != nil && bool(C.hiroz_action_server_feedback_ready(h.p))
		s.mu.RUnlock()
		if h == nil {
			return errors.New("action server is closed")
		}
		if ready {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-s.ctx.Done():
			return errors.New("action server is closed")
		case <-ticker.C:
		}
	}
}
