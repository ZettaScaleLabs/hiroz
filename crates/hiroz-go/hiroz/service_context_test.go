package hiroz

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
	"time"
)

// openTestClient returns a client whose handle is non-nil, so the call path reaches the
// replaceable C seam. No test here touches a real Zenoh session.
func openTestClient() *ServiceClient {
	c := &ServiceClient{service: "/dummy"}
	c.handle.Store(&cServiceClientHandle{})
	return c
}

// stubServiceCall replaces the C call for one test.
func stubServiceCall(t *testing.T, f func(req []byte, timeoutMs uint64) ([]byte, int32)) {
	t.Helper()
	old := serviceCall
	serviceCall = func(_ *cServiceClient, req []byte, timeoutMs uint64) ([]byte, int32) {
		return f(req, timeoutMs)
	}
	t.Cleanup(func() { serviceCall = old })
}

func stubServiceWait(t *testing.T, f func(timeoutMs uint64) int32) {
	t.Helper()
	old := serviceWait
	serviceWait = func(_ *cServiceClient, timeoutMs uint64) int32 { return f(timeoutMs) }
	t.Cleanup(func() { serviceWait = old })
}

func mockReq() *MockMessage { return &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}} }

func TestCallTypedContextCancelReturnsCanceled(t *testing.T) {
	release := make(chan struct{})
	started := make(chan struct{})
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) {
		close(started)
		<-release
		return nil, int32(ErrorCodeServiceTimeout)
	})
	c := openTestClient()
	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan error, 1)
	go func() { done <- CallTypedContext(ctx, c, mockReq(), &MockMessage{}) }()
	<-started
	cancel()

	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("want context.Canceled, got %v", err)
		}
		if errors.Is(err, ErrTimeout) {
			t.Errorf("cancel must not read as a service timeout: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("CallTypedContext did not return after cancel")
	}

	// Close waits for the call that is still inside the C layer.
	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()
	select {
	case <-closed:
		t.Fatal("Close returned while a call was in flight")
	case <-time.After(50 * time.Millisecond):
	}
	close(release)
	select {
	case <-closed:
	case <-time.After(2 * time.Second):
		t.Fatal("Close did not return after the call finished")
	}
}

func TestCallTypedContextDeadlineIsNotServiceTimeout(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) {
		<-release
		return nil, int32(ErrorCodeServiceTimeout)
	})
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()

	err := CallTypedContext(ctx, openTestClient(), mockReq(), &MockMessage{})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("want context.DeadlineExceeded, got %v", err)
	}
	if errors.Is(err, ErrTimeout) {
		t.Errorf("a context deadline must not read as ErrTimeout: %v", err)
	}
}

func TestCallTypedContextPassesDeadlineAsTimeout(t *testing.T) {
	var got atomic.Uint64
	stubServiceCall(t, func(_ []byte, timeoutMs uint64) ([]byte, int32) {
		got.Store(timeoutMs)
		return []byte{7}, 0
	})
	ctx, cancel := context.WithTimeout(context.Background(), 800*time.Millisecond)
	defer cancel()

	resp := &MockMessage{}
	if err := CallTypedContext(ctx, openTestClient(), mockReq(), resp); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ms := got.Load(); ms == 0 || ms > 800 {
		t.Errorf("timeout passed to C = %d ms, want 1..800", ms)
	}
	if len(resp.data) != 1 || resp.data[0] != 7 {
		t.Errorf("response not deserialized: %v", resp.data)
	}
}

func TestCallTypedContextNoDeadlineUsesDefault(t *testing.T) {
	var got atomic.Uint64
	stubServiceCall(t, func(_ []byte, timeoutMs uint64) ([]byte, int32) {
		got.Store(timeoutMs)
		return []byte{7}, 0
	})
	if err := CallTypedContext(context.Background(), openTestClient(), mockReq(), &MockMessage{}); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if want := uint64(DefaultServiceTimeout.Milliseconds()); got.Load() != want {
		t.Errorf("timeout passed to C = %d ms, want %d", got.Load(), want)
	}
}

func TestCallTypedContextServerTimeoutStaysServiceTimeout(t *testing.T) {
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) { return nil, int32(ErrorCodeServiceTimeout) })
	err := CallTypedContext(context.Background(), openTestClient(), mockReq(), &MockMessage{})
	if !errors.Is(err, ErrTimeout) {
		t.Fatalf("want ErrTimeout, got %v", err)
	}
}

func TestCallTypedContextAlreadyCancelledSkipsCall(t *testing.T) {
	var calls atomic.Int32
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) { calls.Add(1); return []byte{1}, 0 })
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	err := CallTypedContext(ctx, openTestClient(), mockReq(), &MockMessage{})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if calls.Load() != 0 {
		t.Error("a cancelled context must not reach the C call")
	}
}

func TestCallTypedContextOnClosedClient(t *testing.T) {
	c := &ServiceClient{service: "/dummy"}
	err := CallTypedContext(context.Background(), c, mockReq(), &MockMessage{})
	if err == nil {
		t.Fatal("expected error on closed client")
	}
	if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("closed client must not read as a context error: %v", err)
	}
}

func TestWaitForServiceContextReady(t *testing.T) {
	var calls atomic.Int32
	stubServiceWait(t, func(timeoutMs uint64) int32 {
		if timeoutMs == 0 || timeoutMs > 250 {
			t.Errorf("slice = %d ms, want 1..250", timeoutMs)
		}
		if calls.Add(1) < 3 {
			return int32(ErrorCodeServiceTimeout)
		}
		return 0
	})
	if err := openTestClient().WaitForServiceContext(context.Background()); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if calls.Load() != 3 {
		t.Errorf("calls = %d, want 3", calls.Load())
	}
}

func TestWaitForServiceContextCancel(t *testing.T) {
	stubServiceWait(t, func(uint64) int32 {
		time.Sleep(5 * time.Millisecond)
		return int32(ErrorCodeServiceTimeout)
	})
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Millisecond)
	defer cancel()
	err := openTestClient().WaitForServiceContext(ctx)
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("want context.DeadlineExceeded, got %v", err)
	}
	if errors.Is(err, ErrTimeout) {
		t.Errorf("a context deadline must not read as ErrTimeout: %v", err)
	}
}

func TestWaitForServiceContextHardError(t *testing.T) {
	stubServiceWait(t, func(uint64) int32 { return int32(ErrorCodeSessionClosed) })
	err := openTestClient().WaitForServiceContext(context.Background())
	var e HirozError
	if !errors.As(err, &e) || e.Code() != ErrorCodeSessionClosed {
		t.Fatalf("want HirozError with SessionClosed, got %v", err)
	}
}

// pastDeadlineCtx reports a deadline that is in the future on the first read and in the
// past after it, while Err stays nil. It models a timer that has not fired yet.
type pastDeadlineCtx struct {
	context.Context
	reads atomic.Int32
}

func (c *pastDeadlineCtx) Deadline() (time.Time, bool) {
	if c.reads.Add(1) == 1 {
		return time.Now().Add(time.Hour), true
	}
	return time.Now().Add(-time.Second), true
}

// A C timeout that returns after the deadline is the context's deadline, not a server timeout.
func TestCallTypedContextTimeoutAfterDeadlineIsDeadlineExceeded(t *testing.T) {
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) { return nil, int32(ErrorCodeServiceTimeout) })
	ctx := &pastDeadlineCtx{Context: context.Background()}
	err := CallTypedContext(ctx, openTestClient(), mockReq(), &MockMessage{})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("want context.DeadlineExceeded, got %v", err)
	}
	if errors.Is(err, ErrTimeout) {
		t.Errorf("a passed deadline must not read as ErrTimeout: %v", err)
	}
}
