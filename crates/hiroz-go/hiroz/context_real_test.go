package hiroz

import (
	"context"
	"errors"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// The tests in this file cancel a call while the real C call is in flight. The other context
// tests replace the C calls with stubs.

// ctxDeliveryMsg decodes into its receiver, like every generated message type. Calling
// DeserializeCDR on a nil *ctxDeliveryMsg dereferences nil.
type ctxDeliveryMsg struct{ Value string }

func (m *ctxDeliveryMsg) TypeName() string { return "test/CtxDeliveryMsg" }
func (m *ctxDeliveryMsg) TypeHash() string {
	return "RIHS01_" + strings.Repeat("cd", 32)
}
func (m *ctxDeliveryMsg) SerializeCDR() ([]byte, error) { return []byte(m.Value), nil }
func (m *ctxDeliveryMsg) DeserializeCDR(data []byte) error {
	m.Value = string(data)
	return nil
}

func realNode(t *testing.T, name string) *Node {
	t.Helper()
	zctx, err := NewContext().DisableMulticastScouting().Build()
	if err != nil {
		t.Fatalf("context: %v", err)
	}
	t.Cleanup(func() { zctx.Close() })
	node, err := zctx.CreateNode(name).Build()
	if err != nil {
		t.Fatalf("node: %v", err)
	}
	t.Cleanup(func() { node.Close() })
	return node
}

// signal reports one event without blocking.
func signal(ch chan<- struct{}) {
	select {
	case ch <- struct{}{}:
	default:
	}
}

// assertCloseWaits starts closeFn and checks that it stays blocked until release runs.
func assertCloseWaits(t *testing.T, closeFn func() error, release func()) {
	t.Helper()
	closed := make(chan error, 1)
	go func() { closed <- closeFn() }()
	select {
	case err := <-closed:
		t.Fatalf("Close returned %v while the C call was in flight", err)
	case <-time.After(blockedWindow):
	}
	release()
	if err := receiveBounded(t, closed, "Close to return after the C call ended"); err != nil {
		t.Errorf("Close: %v", err)
	}
}

func TestCallTypedContextCancelRealLibrary(t *testing.T) {
	node := realNode(t, "ctx_real_call")
	release, free := newRelease(t)
	entered := make(chan struct{}, 1)
	srv, err := node.CreateServiceServer("/ctx_real_call").Build(&mockService{}, func([]byte) ([]byte, error) {
		signal(entered)
		<-release
		return []byte("ok"), nil
	})
	if err != nil {
		t.Fatalf("server: %v", err)
	}
	t.Cleanup(func() { free(); srv.Close() })
	client, err := node.CreateServiceClient("/ctx_real_call").Build(&mockService{})
	if err != nil {
		t.Fatalf("client: %v", err)
	}
	wctx, wcancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer wcancel()
	if err := client.WaitForServiceContext(wctx); err != nil {
		t.Fatalf("the server did not become visible: %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- CallTypedContext(ctx, client, &ctxDeliveryMsg{Value: "q"}, &ctxDeliveryMsg{}) }()
	receiveBounded(t, entered, "the server to receive the request")
	cancel()
	if err := receiveBounded(t, done, "the call to return after cancel"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	assertCloseWaits(t, client.Close, free)
}

func TestWaitForServiceContextCancelRealLibrary(t *testing.T) {
	node := realNode(t, "ctx_real_wait")
	client, err := node.CreateServiceClient("/ctx_real_absent").Build(&mockService{})
	if err != nil {
		t.Fatalf("client: %v", err)
	}
	t.Cleanup(func() { client.Close() })

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- client.WaitForServiceContext(ctx) }()
	select {
	case err := <-done:
		t.Fatalf("the wait returned %v with no server and a live context", err)
	case <-time.After(2 * waitSlice):
	}
	start := time.Now()
	cancel()
	if err := receiveBounded(t, done, "the wait to return after cancel"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if el := time.Since(start); el > 2*waitSlice {
		t.Errorf("the wait returned %v after cancel, want at most one slice of %v plus slack", el, waitSlice)
	}
}

// realActionServer serves /<name>. The goal callback holds while holdGoals is set, and the
// execute callback holds while holdResults is set, until release.
func realActionServer(t *testing.T, node *Node, name string, holdGoals, holdResults *atomic.Bool) (entered chan struct{}, free func()) {
	t.Helper()
	release, free := newRelease(t)
	entered = make(chan struct{}, 1)
	srv, err := node.CreateActionServer(name).Build(&mockAction{},
		func([]byte) bool {
			if holdGoals.Load() {
				signal(entered)
				<-release
			}
			return true
		},
		func(*ServerGoalHandle, []byte) ([]byte, error) {
			if holdResults.Load() {
				signal(entered)
				<-release
			}
			return []byte{0, 1, 0, 0, 1}, nil
		})
	if err != nil {
		t.Fatalf("action server: %v", err)
	}
	t.Cleanup(func() { free(); srv.Close() })
	return entered, free
}

// sendWarmGoal sends goals until the server answers one, so later calls reach a matched server.
func sendWarmGoal(t *testing.T, c *ActionClient) *GoalHandle {
	t.Helper()
	deadline := time.Now().Add(8 * time.Second)
	for {
		h, err := c.SendGoal(&ctxDeliveryMsg{Value: "g"})
		if err == nil {
			return h
		}
		if time.Now().After(deadline) {
			t.Fatalf("no goal reached the server: %v", err)
		}
		time.Sleep(100 * time.Millisecond)
	}
}

func TestSendGoalContextCancelRealLibrary(t *testing.T) {
	node := realNode(t, "ctx_real_send")
	var holdGoals, holdResults atomic.Bool
	entered, free := realActionServer(t, node, "/ctx_real_send", &holdGoals, &holdResults)
	client, err := node.CreateActionClient("/ctx_real_send").Build(&mockAction{})
	if err != nil {
		t.Fatalf("action client: %v", err)
	}
	_ = sendWarmGoal(t, client).Close()

	holdGoals.Store(true)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { _, err := client.SendGoalContext(ctx, &ctxDeliveryMsg{Value: "g"}); done <- err }()
	receiveBounded(t, entered, "the server to receive the goal")
	cancel()
	if err := receiveBounded(t, done, "the send to return after cancel"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	assertCloseWaits(t, client.Close, free)
}

func TestGetResultWithContextCancelRealLibrary(t *testing.T) {
	node := realNode(t, "ctx_real_result")
	var holdGoals, holdResults atomic.Bool
	holdResults.Store(true)
	entered, free := realActionServer(t, node, "/ctx_real_result", &holdGoals, &holdResults)
	client, err := node.CreateActionClient("/ctx_real_result").Build(&mockAction{})
	if err != nil {
		t.Fatalf("action client: %v", err)
	}
	h := sendWarmGoal(t, client)
	defer h.Close()
	receiveBounded(t, entered, "the server to execute the goal")

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { _, err := h.GetResultWithContext(ctx); done <- err }()
	// Give the C wait time to start. A cancel that comes first also returns context.Canceled,
	// but then Close does not block, and assertCloseWaits fails.
	time.Sleep(blockedWindow)
	cancel()
	if err := receiveBounded(t, done, "the result wait to return after cancel"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	assertCloseWaits(t, client.Close, free)
}
