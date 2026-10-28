package hiroz

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"math"
	"sync"
	"testing"
	"time"
)

// The fixture deliberately mixes alignments instead of repeating Fibonacci's
// uint32-aligned layout. These methods are the generated codec contract.
type actionFixture struct {
	Tag   byte
	Code  uint16
	Value float64
}

func (*actionFixture) TypeName() string { return "action_test/msg/Fixture" }
func (*actionFixture) TypeHash() string {
	return "RIHS01_0000000000000000000000000000000000000000000000000000000000000001"
}
func (m *actionFixture) SerializeCDR() ([]byte, error) {
	b, _ := m.PackToRawAt(cdrHeader(), 0)
	return b, nil
}
func (m *actionFixture) DeserializeCDR(b []byte) error {
	if err := checkCDR(b, 4); err != nil {
		return err
	}
	_, err := m.UnpackFromRawAt(b[4:], 0)
	return err
}
func (m *actionFixture) PackToRawAt(b []byte, p int) ([]byte, int) {
	b = append(b, m.Tag)
	p++
	pad := (2 - p%2) % 2
	b = append(b, make([]byte, pad)...)
	p += pad
	b = binary.LittleEndian.AppendUint16(b, m.Code)
	p += 2
	pad = (8 - p%8) % 8
	b = append(b, make([]byte, pad)...)
	p += pad
	b = binary.LittleEndian.AppendUint64(b, math.Float64bits(m.Value))
	return b, p + 8
}
func (m *actionFixture) UnpackFromRawAt(b []byte, p int) (int, error) {
	if p >= len(b) {
		return p, errors.New("short fixture")
	}
	m.Tag = b[p]
	p++
	p += (2 - p%2) % 2
	if p+2 > len(b) {
		return p, errors.New("short fixture")
	}
	m.Code = binary.LittleEndian.Uint16(b[p:])
	p += 2
	p += (8 - p%8) % 8
	if p+8 > len(b) {
		return p, errors.New("short fixture")
	}
	m.Value = math.Float64frombits(binary.LittleEndian.Uint64(b[p:]))
	return p + 8, nil
}

type fixtureAction struct{}

func (*fixtureAction) TypeName() string      { return "action_test/action/Fixture" }
func (*fixtureAction) GetGoal() Message      { return &actionFixture{} }
func (*fixtureAction) GetResult() Message    { return &actionFixture{} }
func (*fixtureAction) GetFeedback() Message  { return &actionFixture{} }
func (*fixtureAction) SendGoalHash() string  { return (&actionFixture{}).TypeHash() }
func (*fixtureAction) GetResultHash() string { return (&actionFixture{}).TypeHash() }
func (*fixtureAction) CancelGoalHash() string {
	return "RIHS01_573d8b0a534451d7bc2ac8c5ffde8ac14b8593b7001175d0cd6516dcbeb8689a"
}
func (*fixtureAction) FeedbackMessageHash() string { return (&actionFixture{}).TypeHash() }
func (*fixtureAction) StatusHash() string {
	return "RIHS01_6c1684b00f177d37438febe6e709fc4e2b0d4248dca4854946f9ed8b30cda83e"
}

func TestActionResultEnvelopeUsesAbsoluteOffsets(t *testing.T) {
	message := &actionFixture{Tag: 1, Code: 0x1234, Value: 3.5}
	envelope, err := encodeResult(message, GoalStatusAborted)
	if err != nil {
		t.Fatal(err)
	}
	// [status][tag][uint16][four padding][float64], measured from CDR body.
	expected := []byte{0, 1, 0, 0, 6, 1, 0x34, 0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0c, 0x40}
	if !bytes.Equal(envelope, expected) {
		t.Fatalf("got %x, want %x", envelope, expected)
	}
	decoded := &actionFixture{}
	if _, err := decoded.UnpackFromRawAt(envelope[4:], 1); err != nil {
		t.Fatal(err)
	}
	if *decoded != *message {
		t.Fatalf("got %#v, want %#v", decoded, message)
	}
	if _, err := encodeResult(&MockMessage{}, GoalStatusSucceeded); err == nil {
		t.Fatal("opaque codec should be rejected, never alignment-guessed")
	}
}

func actionTestNode(t *testing.T) *Node {
	t.Helper()
	ctx, err := NewContext().WithMode(ModePeer).DisableMulticastScouting().WithJSON(`{"connect/endpoints":[],"listen/endpoints": [], "scouting/gossip/enabled": false}`).Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode(fmt.Sprintf("action_test_%d", time.Now().UnixNano())).Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { node.Close() })
	return node
}
func actionTestClient(t *testing.T, node *Node, name string) *ActionClient {
	t.Helper()
	client, err := node.CreateActionClient(name).Build(&fixtureAction{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { client.Close() })
	return client
}
func resultWithDeadline(t *testing.T, h *GoalHandle) (GoalStatus, *actionFixture) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	status, data, err := h.GetResultWithStatus(ctx)
	if err != nil {
		t.Fatal(err)
	}
	result := &actionFixture{}
	if err = result.DeserializeCDR(data); err != nil {
		t.Fatal(err)
	}
	return status, result
}

func TestActionLiveTerminalResultsFeedbackAndMultipleWaiters(t *testing.T) {
	node := actionTestNode(t)
	for _, outcome := range []GoalStatus{GoalStatusSucceeded, GoalStatusAborted} {
		t.Run(outcome.String(), func(t *testing.T) {
			release := make(chan struct{})
			started := make(chan *ServerGoalHandle, 1)
			server, err := node.CreateActionServer("terminal_"+outcome.String()).Build(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, _ []byte) ([]byte, error) {
				started <- h
				select {
				case <-release:
				case <-h.Context().Done():
					return nil, h.Context().Err()
				}
				if err := h.PublishFeedback(&actionFixture{Code: 31, Value: 1.5}); err != nil {
					return nil, err
				}
				result := &actionFixture{Code: 42, Value: 2.5}
				if outcome == GoalStatusAborted {
					if err := h.Abort(result); err != nil {
						return nil, err
					}
					return (&actionFixture{Code: 99}).SerializeCDR()
				}
				return result.SerializeCDR()
			})
			if err != nil {
				t.Fatal(err)
			}
			defer server.Close()
			client := actionTestClient(t, node, "terminal_"+outcome.String())
			h, err := client.SendGoal(&actionFixture{Code: 5})
			if err != nil {
				t.Fatal(err)
			}
			defer h.Close()
			select {
			case <-started:
			case <-time.After(time.Second):
				t.Fatal("execution did not start")
			}
			type reply struct {
				status GoalStatus
				data   []byte
				err    error
			}
			replies := make(chan reply, 2)
			for i := 0; i < 2; i++ {
				go func() {
					ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
					defer cancel()
					s, d, e := h.GetResultWithStatus(ctx)
					replies <- reply{s, d, e}
				}()
			}
			// Waiters are registered before completion to catch single-query caches.
			time.Sleep(30 * time.Millisecond)
			close(release)
			for i := 0; i < 2; i++ {
				r := <-replies
				if r.err != nil {
					t.Fatal(r.err)
				}
				if r.status != outcome {
					t.Fatalf("status=%v want %v", r.status, outcome)
				}
				m := &actionFixture{}
				if err := m.DeserializeCDR(r.data); err != nil {
					t.Fatal(err)
				}
				if m.Code != 42 {
					t.Fatalf("explicit result overwritten: %+v", m)
				}
			}
			select {
			case data := <-h.Feedback():
				m := &actionFixture{}
				if err := m.DeserializeCDR(data); err != nil || m.Code != 31 {
					t.Fatalf("feedback %+v %v", m, err)
				}
			case <-time.After(time.Second):
				t.Fatal("client did not receive feedback")
			}
			if h.Status() != outcome {
				t.Fatalf("live status=%v", h.Status())
			}
		})
	}
}

func TestActionLiveCancelPolicySelectionAndDeferredExecution(t *testing.T) {
	node := actionTestNode(t)
	accepted := make(chan *ServerGoalHandle, 4)
	server, err := node.CreateActionServer("cancel_policy").WithCancelCallback(func(h *ServerGoalHandle) bool { return true }).BuildDeferred(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, _ []byte) { accepted <- h })
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client := actionTestClient(t, node, "cancel_policy")
	first, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	sh1 := <-accepted
	second, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer second.Close()
	sh2 := <-accepted
	if sh1.Status() != GoalStatusAccepted {
		t.Fatal("deferred goal started executing")
	}
	if err := sh1.Succeed(&actionFixture{}); err == nil {
		t.Fatal("deferred goal completed before Execute")
	}
	if err := sh1.PublishFeedback(&actionFixture{}); err == nil {
		t.Fatal("deferred goal published before Execute")
	}
	if err := sh1.Execute(); err != nil {
		t.Fatal(err)
	}
	response, err := first.CancelWithContext(context.Background())
	if err != nil || response.ReturnCode != CancelAccepted || len(response.GoalsCanceling) != 1 {
		t.Fatalf("cancel=%+v %v", response, err)
	}
	if sh2.IsCancelRequested() {
		t.Fatal("single-goal cancellation selected unrelated goal")
	}
	if err := sh1.Canceled(&actionFixture{Code: 17}); err != nil {
		t.Fatal(err)
	}
	status, result := resultWithDeadline(t, first)
	if status != GoalStatusCanceled || result.Code != 17 {
		t.Fatalf("canceled result %v %+v", status, result)
	}
	response, err = first.CancelWithContext(context.Background())
	if err != nil || response.ReturnCode != CancelGoalTerminated {
		t.Fatalf("terminal cancel=%+v %v", response, err)
	}
	response, err = client.CancelGoals(context.Background(), GoalID{123}, time.Time{})
	if err != nil || response.ReturnCode != CancelUnknownGoal {
		t.Fatalf("unknown cancel=%+v %v", response, err)
	}
	response, err = client.CancelAllGoals(context.Background())
	if err != nil || len(response.GoalsCanceling) != 1 || !sh2.IsCancelRequested() {
		t.Fatalf("all cancel=%+v %v", response, err)
	}
	if err := sh2.Canceled(&actionFixture{}); err != nil {
		t.Fatal(err)
	}
}

func TestActionLiveRejectedCancelFailureExpiryAndActiveClose(t *testing.T) {
	node := actionTestNode(t)
	server, err := node.CreateActionServer("reject").WithResultTimeout(40*time.Millisecond).WithCancelCallback(func(*ServerGoalHandle) bool { return false }).Build(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, _ []byte) ([]byte, error) {
		<-h.Context().Done()
		return nil, errors.New("execution failed")
	})
	if err != nil {
		t.Fatal(err)
	}
	client := actionTestClient(t, node, "reject")
	goal, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	response, err := goal.CancelWithContext(context.Background())
	if err != nil || response.ReturnCode != CancelRejected || goal.Status() == GoalStatusCanceling {
		t.Fatalf("rejected cancel=%+v %v", response, err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { _, err := goal.GetResultWithContext(ctx); done <- err }()
	time.Sleep(10 * time.Millisecond)
	cancel()
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("context did not interrupt result request")
	}
	var wg sync.WaitGroup
	wg.Add(1)
	go func() { defer wg.Done(); goal.GetResult() }()
	time.Sleep(10 * time.Millisecond)
	closeDone := make(chan struct{})
	go func() { goal.Close(); client.Close(); server.Close(); wg.Wait(); close(closeDone) }()
	select {
	case <-closeDone:
	case <-time.After(time.Second):
		t.Fatal("close did not cancel and drain active operations")
	}
	expired, err := node.CreateActionServer("expiry").WithResultTimeout(40*time.Millisecond).Build(&fixtureAction{}, func([]byte) bool { return true }, func(*ServerGoalHandle, []byte) ([]byte, error) { return nil, errors.New("failed execution") })
	if err != nil {
		t.Fatal(err)
	}
	defer expired.Close()
	ec := actionTestClient(t, node, "expiry")
	eh, err := ec.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer eh.Close()
	status, _ := resultWithDeadline(t, eh)
	if status != GoalStatusAborted {
		t.Fatalf("error callback status=%v", status)
	}
	time.Sleep(80 * time.Millisecond)
	ctx2, cancel2 := context.WithTimeout(context.Background(), time.Second)
	defer cancel2()
	status, _, err = eh.GetResultWithStatus(ctx2)
	if err == nil || status != GoalStatusUnknown {
		t.Fatalf("expired result=%v %v", status, err)
	}
}

func TestActionLiveResultBeyondOldTransportDeadline(t *testing.T) {
	if testing.Short() {
		t.Skip("12-second transport deadline regression")
	}
	node := actionTestNode(t)
	server, err := node.CreateActionServer("long_result").Build(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, _ []byte) ([]byte, error) {
		select {
		case <-time.After(12 * time.Second):
			return (&actionFixture{Code: 120}).SerializeCDR()
		case <-h.Context().Done():
			return nil, h.Context().Err()
		}
	})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client := actionTestClient(t, node, "long_result")
	goal, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer goal.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	status, data, err := goal.GetResultWithStatus(ctx)
	if err != nil || status != GoalStatusSucceeded {
		t.Fatalf("long action result=%v %v", status, err)
	}
	result := &actionFixture{}
	if err := result.DeserializeCDR(data); err != nil || result.Code != 120 {
		t.Fatalf("long action payload=%+v %v", result, err)
	}
}

func TestActionLiveGoalTimeoutAndMetadata(t *testing.T) {
	for _, timeout := range []time.Duration{100 * time.Millisecond, 500 * time.Microsecond, time.Nanosecond} {
		t.Run(timeout.String(), func(t *testing.T) {
			node := actionTestNode(t)
			accepted := make(chan *ServerGoalHandle, 1)
			ids := make(chan GoalID, 1)
			server, err := node.CreateActionServer("goal_expiration").WithGoalTimeout(timeout).WithGoalCallback(func(id GoalID, _ []byte) bool { ids <- id; return true }).BuildDeferred(&fixtureAction{}, nil, func(h *ServerGoalHandle, _ []byte) { accepted <- h })
			if err != nil {
				t.Fatal(err)
			}
			defer server.Close()
			client := actionTestClient(t, node, "goal_expiration")
			wait, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			if err := server.WaitForFeedbackSubscriber(wait); err != nil {
				t.Fatal(err)
			}
			goal, err := client.SendGoal(&actionFixture{})
			if err != nil {
				t.Fatal(err)
			}
			defer goal.Close()
			serverGoal := <-accepted
			info, err := serverGoal.Info()
			if err != nil || info.GoalID != goal.GoalID() || info.Stamp.IsZero() {
				t.Fatalf("metadata=%+v %v", info, err)
			}
			if id := <-ids; id != goal.GoalID() {
				t.Fatal("goal callback received wrong UUID")
			}
			status, _ := resultWithDeadline(t, goal)
			if status != GoalStatusAborted {
				t.Fatalf("goal expiration status=%v", status)
			}
		})
	}
}

func TestActionLiveServerCloseFromCallback(t *testing.T) {
	node := actionTestNode(t)
	returned := make(chan struct{})
	server, err := node.CreateActionServer("callback_close").Build(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, _ []byte) ([]byte, error) {
		if err := h.server.Close(); err != nil {
			return nil, err
		}
		close(returned)
		return (&actionFixture{}).SerializeCDR()
	})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client := actionTestClient(t, node, "callback_close")
	goal, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer goal.Close()
	select {
	case <-returned:
	case <-time.After(time.Second):
		t.Fatal("Close deadlocked inside execute callback")
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := server.WaitClosed(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestActionServerCallbackOutlivesParentsAndClosesSafely(t *testing.T) {
	parent, err := NewContext().DisableMulticastScouting().WithJSON(`{"connect/endpoints":[],"listen/endpoints":[],"scouting/gossip/enabled":false,"scouting/delay":0}`).Build()
	if err != nil {
		t.Fatal(err)
	}
	defer parent.Close()
	node, err := parent.CreateNode("action_parent_lifetime").Build()
	if err != nil {
		t.Fatal(err)
	}
	defer node.Close()
	started := make(chan struct{})
	parentsClosed := make(chan struct{})
	release := make(chan struct{})
	reported := make(chan error, 1)
	var resumeOnce, releaseOnce sync.Once
	server, err := node.CreateActionServer("parent_lifetime").Build(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, _ []byte) ([]byte, error) {
		close(started)
		<-parentsClosed
		feedbackErr := h.PublishFeedback(&actionFixture{Code: 42})
		_ = h.server.Close()
		reported <- feedbackErr
		<-release
		return nil, h.Context().Err()
	})
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		resumeOnce.Do(func() { close(parentsClosed) })
		releaseOnce.Do(func() { close(release) })
		_ = server.Close()
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if err := server.WaitClosed(ctx); err != nil {
			t.Errorf("callback cleanup: %v", err)
		}
	}()
	client := actionTestClient(t, node, "parent_lifetime")
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	goal, err := client.SendGoalWithContext(ctx, &actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer goal.Close()
	select {
	case <-started:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	// Remove every other session owner while the native execute callback is live.
	_ = goal.Close()
	_ = client.Close()
	_ = node.Close()
	_ = parent.Close()
	resumeOnce.Do(func() { close(parentsClosed) })
	select {
	case err := <-reported:
		if err != nil {
			t.Fatalf("live callback lost its parent session: %v", err)
		}
	case <-ctx.Done():
		t.Fatal("Close deadlocked inside the callback after parent closure")
	}
	select {
	case <-server.closedDone:
		t.Fatal("native teardown finished before the callback returned")
	default:
	}
	// Match the generated-suite failure: the next context starts while the
	// preceding action server is still draining asynchronous callback teardown.
	next, err := NewContext().DisableMulticastScouting().WithJSON(`{"connect/endpoints":[],"listen/endpoints":[],"scouting/gossip/enabled":false,"scouting/delay":0}`).Build()
	if err != nil {
		t.Fatal(err)
	}
	defer next.Close()
	releaseOnce.Do(func() { close(release) })
	if err := server.WaitClosed(ctx); err != nil {
		t.Fatal(err)
	}
}

func TestActionLiveCanceledBeforeDispatchAndClosedGoal(t *testing.T) {
	node := actionTestNode(t)
	seen := make(chan struct{}, 1)
	server, err := node.CreateActionServer("dispatch_gate").Build(&fixtureAction{}, func([]byte) bool { seen <- struct{}{}; return true }, func(*ServerGoalHandle, []byte) ([]byte, error) { return (&actionFixture{}).SerializeCDR() })
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client := actionTestClient(t, node, "dispatch_gate")
	client.mu.Lock()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		h, err := client.SendGoalWithContext(ctx, &actionFixture{})
		if h != nil {
			h.Close()
		}
		done <- err
	}()
	time.Sleep(20 * time.Millisecond)
	cancel()
	client.mu.Unlock()
	if err := <-done; !errors.Is(err, context.Canceled) {
		t.Fatalf("dispatch result=%v", err)
	}
	select {
	case <-seen:
		t.Fatal("canceled goal dispatched after waiting for mutex")
	case <-time.After(20 * time.Millisecond):
	}
	goal, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	resultWithDeadline(t, goal)
	goal.Close()
	if _, err := goal.GetResult(); err == nil {
		t.Fatal("closed goal returned a cached remote result")
	}
}

// Scalar fixtures model distinct generated result layouts at the action boundary.
type scalarResult struct {
	width int
	value uint64
}

func (*scalarResult) TypeName() string { return "action_test/msg/Scalar" }
func (*scalarResult) TypeHash() string { return (&actionFixture{}).TypeHash() }
func (m *scalarResult) PackToRawAt(b []byte, p int) ([]byte, int) {
	pad := (m.width - p%m.width) % m.width
	b = append(b, make([]byte, pad)...)
	for i := 0; i < m.width; i++ {
		b = append(b, byte(m.value>>(8*i)))
	}
	return b, p + pad + m.width
}
func (m *scalarResult) UnpackFromRawAt(b []byte, p int) (int, error) {
	p += (m.width - p%m.width) % m.width
	if p+m.width > len(b) {
		return p, errors.New("short scalar")
	}
	m.value = 0
	for i := 0; i < m.width; i++ {
		m.value |= uint64(b[p+i]) << (8 * i)
	}
	return p + m.width, nil
}
func (m *scalarResult) SerializeCDR() ([]byte, error) {
	b, _ := m.PackToRawAt(cdrHeader(), 0)
	return b, nil
}
func (m *scalarResult) DeserializeCDR(b []byte) error {
	if err := checkCDR(b, 4); err != nil {
		return err
	}
	_, err := m.UnpackFromRawAt(b[4:], 0)
	return err
}
func TestActionResultBooleanUint16AndFloat64Layouts(t *testing.T) {
	cases := []struct {
		name     string
		message  scalarResult
		expected []byte
	}{
		{"boolean", scalarResult{1, 1}, []byte{0, 1, 0, 0, 4, 1}},
		{"uint16", scalarResult{2, 0x1234}, []byte{0, 1, 0, 0, 4, 0, 0x34, 0x12}},
		{"float64", scalarResult{8, math.Float64bits(3.5)}, []byte{0, 1, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0c, 0x40}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			b, err := encodeResult(&tc.message, GoalStatusSucceeded)
			if err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(b, tc.expected) {
				t.Fatalf("got %x, want %x", b, tc.expected)
			}
			decoded := &scalarResult{width: tc.message.width}
			if _, err := decoded.UnpackFromRawAt(b[4:], 1); err != nil || decoded.value != tc.message.value {
				t.Fatalf("decoded=%+v %v", decoded, err)
			}
		})
	}
}

func TestActionLiveStatusWithoutResultRequests(t *testing.T) {
	node := actionTestNode(t)
	accepted := make(chan *ServerGoalHandle, 2)
	server, err := node.CreateActionServer("status_only").BuildDeferred(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, _ []byte) { accepted <- h })
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client := actionTestClient(t, node, "status_only")
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if err := client.WaitForServer(ctx); err != nil {
		t.Fatal(err)
	}
	first, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	a := <-accepted
	second, err := client.SendGoal(&actionFixture{})
	if err != nil {
		t.Fatal(err)
	}
	defer second.Close()
	b := <-accepted
	if err := a.Execute(); err != nil {
		t.Fatal(err)
	}
	if err := b.Execute(); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(time.Second)
	for (first.Status() != GoalStatusExecuting || second.Status() != GoalStatusExecuting) && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	if first.Status() != GoalStatusExecuting || second.Status() != GoalStatusExecuting {
		t.Fatalf("status topic did not update both goals: %v/%v", first.Status(), second.Status())
	}
	if err := a.Abort(&actionFixture{}); err != nil {
		t.Fatal(err)
	}
	if err := b.Succeed(&actionFixture{}); err != nil {
		t.Fatal(err)
	}
	deadline = time.Now().Add(time.Second)
	for (!first.IsTerminal() || !second.IsTerminal()) && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	if first.Status() != GoalStatusAborted || second.Status() != GoalStatusSucceeded {
		t.Fatalf("terminal status topic failed: %v/%v", first.Status(), second.Status())
	}
}

func TestActionCanceledFeedbackWaitPreservesBufferedSample(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	payload, err := (&actionFixture{Code: 42}).SerializeCDR()
	if err != nil {
		t.Fatal(err)
	}
	// Both a queued sample and Done are ready. A select alone may choose the
	// sample and incorrectly report success for an already canceled caller.
	for i := 0; i < 32; i++ {
		feedback := make(chan []byte, 1)
		feedback <- payload
		goal := &GoalHandle{feedback: feedback}
		err := ReceiveTypedFeedback(ctx, goal, &actionFixture{})
		if !errors.Is(err, context.Canceled) || len(feedback) != 1 {
			t.Fatalf("canceled feedback wait: error=%v buffered=%d", err, len(feedback))
		}
	}
}

// The Go server must follow the same ROS UUID/time selection as native servers.
// rcl_action treats an empty timestamp-bearing selection as a successful no-op;
// unknown/terminal UUID errors apply only to a zero-timestamp UUID request.
func TestActionLiveCancelTimestampSelectionAndEmptyMatches(t *testing.T) {
	for mode, name := range []string{"all", "uuid", "cutoff", "uuid_plus_cutoff"} {
		t.Run(name, func(t *testing.T) {
			node := actionTestNode(t)
			accepted := make(chan *ServerGoalHandle, 3)
			server, err := node.CreateActionServer("timestamp_selection").BuildDeferred(
				&fixtureAction{}, func([]byte) bool { return true },
				func(h *ServerGoalHandle, _ []byte) { accepted <- h })
			if err != nil {
				t.Fatal(err)
			}
			defer server.Close()
			client := actionTestClient(t, node, "timestamp_selection")
			ctx, stop := context.WithTimeout(context.Background(), 5*time.Second)
			defer stop()
			if err := client.WaitForServer(ctx); err != nil {
				t.Fatal(err)
			}
			var goals []*GoalHandle
			var handles []*ServerGoalHandle
			for i := range 3 {
				goal, err := client.SendGoalWithContext(ctx, &actionFixture{Code: uint16(i)})
				if err != nil {
					t.Fatal(err)
				}
				defer goal.Close()
				goals = append(goals, goal)
				select {
				case handle := <-accepted:
					if handle.GoalID() != goal.GoalID() {
						t.Fatal("accepted wrong goal")
					}
					handles = append(handles, handle)
				case <-ctx.Done():
					t.Fatal(ctx.Err())
				}
				if i > 0 && !goal.Info().Stamp.After(goals[i-1].Info().Stamp) {
					t.Fatal("goal acceptance times must be ordered for cutoff regression")
				}
			}
			var id GoalID
			var cutoff time.Time
			if mode%2 == 1 {
				id = goals[2].GoalID()
			}
			if mode >= 2 {
				cutoff = goals[0].Info().Stamp
			}
			want := map[GoalID]bool{}
			for i, goal := range goals {
				if mode == 0 || (mode%2 == 1 && i == 2) || (mode >= 2 && i == 0) {
					want[goal.GoalID()] = true
				}
			}
			response, err := client.CancelGoals(ctx, id, cutoff)
			if err != nil || response.ReturnCode != CancelAccepted || len(response.GoalsCanceling) != len(want) {
				t.Fatalf("selection: response=%+v err=%v want %d goals", response, err, len(want))
			}
			for _, info := range response.GoalsCanceling {
				if !want[info.GoalID] {
					t.Fatalf("canceled unrelated goal: %+v", info)
				}
				for _, goal := range goals {
					if goal.GoalID() == info.GoalID && !goal.Info().Stamp.Equal(info.Stamp) {
						t.Fatalf("changed acceptance time: got %v want %v", info.Stamp, goal.Info().Stamp)
					}
				}
			}
			for i, handle := range handles {
				if handle.IsCancelRequested() != want[goals[i].GoalID()] {
					t.Fatalf("wrong cancellation state for goal %d", i)
				}
			}
			if handles[0].IsCancelRequested() {
				err = handles[0].Canceled(&actionFixture{})
			} else {
				if err = handles[0].Execute(); err == nil {
					err = handles[0].Succeed(&actionFixture{})
				}
			}
			if err != nil {
				t.Fatal(err)
			}
			// The known UUID is now terminal; the other UUID is unknown. The
			// nonzero cutoff predates every acceptance, so all three are empty
			// timestamp selections rather than UUID-only errors.
			before := goals[0].Info().Stamp.Add(-time.Nanosecond)
			for _, id := range []GoalID{{}, {99}, goals[0].GoalID()} {
				response, err := client.CancelGoals(ctx, id, before)
				if err != nil || response.ReturnCode != CancelAccepted || len(response.GoalsCanceling) != 0 {
					t.Fatalf("empty timestamp selection: response=%+v err=%v", response, err)
				}
			}
		})
	}
}
