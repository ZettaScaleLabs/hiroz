package hiroz

import (
	"context"
	"errors"
	"runtime"
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
	release, free := newRelease(t)
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
	receiveBounded(t, started, "the C layer to start")
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
	free()
	select {
	case <-closed:
	case <-time.After(2 * time.Second):
		t.Fatal("Close did not return after the call finished")
	}
}

func TestCallTypedContextDeadlineIsNotServiceTimeout(t *testing.T) {
	release, free := newRelease(t)
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) {
		<-release
		return nil, int32(ErrorCodeServiceTimeout)
	})
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()

	c := openTestClient()
	err := CallTypedContext(ctx, c, mockReq(), &MockMessage{})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("want context.DeadlineExceeded, got %v", err)
	}
	if errors.Is(err, ErrTimeout) {
		t.Errorf("a context deadline must not read as ErrTimeout: %v", err)
	}

	// Let the call that is still in the C layer finish before the stub is restored.
	free()
	_ = c.Close()
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
	// The deadline was 800 ms when the call began. Only the time the call itself took is gone.
	if ms := got.Load(); ms < 600 || ms > 800 {
		t.Errorf("timeout passed to C = %d ms, want 600..800", ms)
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
	var slices atomic.Int32
	stubServiceWait(t, func(uint64) int32 {
		slices.Add(1)
		runtime.Gosched()
		return int32(ErrorCodeServiceTimeout)
	})
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Millisecond)
	defer cancel()
	err := openTestClient().WaitForServiceContext(ctx)
	if slices.Load() == 0 {
		t.Fatal("the deadline passed before the first wait slice ran, so the test did not exercise a wait")
	}
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

// fixedDeadlineCtx has one fixed deadline and an Err that stays nil. It models a timer that
// has not fired yet when the C call returns.
type fixedDeadlineCtx struct {
	context.Context
	dl time.Time
}

func (c fixedDeadlineCtx) Deadline() (time.Time, bool) { return c.dl, true }

// A C timeout that returns after the deadline is the context's deadline, not a server timeout.
// The stub returns the C timeout only after the deadline has passed, while Err is still nil.
// A slow machine can pass the deadline before the call starts. The test then retries, because
// only a run in which the stub ran exercises the mapping.
func TestCallTypedContextTimeoutAfterDeadlineIsDeadlineExceeded(t *testing.T) {
	for attempt := 0; attempt < 3; attempt++ {
		dl := time.Now().Add(300 * time.Millisecond)
		var ran atomic.Bool
		stubServiceCall(t, func([]byte, uint64) ([]byte, int32) {
			ran.Store(true)
			time.Sleep(time.Until(dl) + 2*time.Millisecond)
			return nil, int32(ErrorCodeServiceTimeout)
		})
		ctx := fixedDeadlineCtx{Context: context.Background(), dl: dl}
		err := CallTypedContext(ctx, openTestClient(), mockReq(), &MockMessage{})
		if !ran.Load() {
			continue
		}
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatalf("want context.DeadlineExceeded, got %v", err)
		}
		if errors.Is(err, ErrTimeout) {
			t.Errorf("a passed deadline must not read as ErrTimeout: %v", err)
		}
		return
	}
	t.Fatal("the deadline passed before the call started in every attempt")
}

// A server that becomes visible while the context ends makes the C wait return 0. The
// context ended first, so the call returns the context error and not success.
func TestWaitForServiceContextCancelDuringWaitWinsOverReady(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	stubServiceWait(t, func(uint64) int32 {
		cancel()
		return 0
	})
	err := openTestClient().WaitForServiceContext(ctx)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
}

// Close that starts while the C wait runs must not read as a ready service when the wait
// returns 0 afterwards.
func TestWaitForServiceContextCloseDuringWaitWinsOverReady(t *testing.T) {
	c := openTestClient()
	closed := make(chan struct{})
	stubServiceWait(t, func(uint64) int32 {
		go func() { _ = c.Close(); close(closed) }()
		receiveBounded(t, c.inflight.quitting(), "Close to start")
		return 0
	})
	err := c.WaitForServiceContext(context.Background())
	if err == nil {
		t.Fatal("want an error that says the client is closed, got success")
	}
	if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("close must not read as a context error: %v", err)
	}
	receiveBounded(t, closed, "Close to return")
}

// A request that returns shared storage must not change what the C layer sends after the call
// returned on cancel. callRawContext copies the bytes before the worker starts.
func TestCallTypedContextOwnsSerialisedBytes(t *testing.T) {
	c := openTestClient()
	req := &sharedGoal{data: []byte{1, 2}}
	started := make(chan struct{})
	release, free := newRelease(t)
	sent := make(chan []byte, 1)
	stubServiceCall(t, func(b []byte, _ uint64) ([]byte, int32) {
		close(started)
		<-release
		sent <- append([]byte(nil), b...) // the C layer reads the request bytes here
		return nil, 1
	})
	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan error, 1)
	go func() { done <- CallTypedContext(ctx, c, req, &MockMessage{}) }()
	receiveBounded(t, started, "the C layer to start")
	cancel()
	if err := receiveBounded(t, done, "the call to return"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	req.data[0] = 9 // the caller reuses the request storage in place
	free()
	if got := receiveBounded(t, sent, "the C layer to send"); len(got) != 2 || got[0] != 1 || got[1] != 2 {
		t.Errorf("the C layer sent %v, want the request as it was at the call: [1 2]", got)
	}
	_ = c.Close()
}

// An ended context wins over a closed client, as it does in the call variants.
func TestWaitForServiceContextCancelledBeatsClosedClient(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	never := &ServiceClient{service: "/dummy"}
	if err := never.WaitForServiceContext(ctx); !errors.Is(err, context.Canceled) {
		t.Errorf("client without a handle: want context.Canceled, got %v", err)
	}

	closed := openTestClient()
	_ = closed.Close()
	if err := closed.WaitForServiceContext(ctx); !errors.Is(err, context.Canceled) {
		t.Errorf("closed client: want context.Canceled, got %v", err)
	}
}

// Positive control: with a live context a closed client reports that it is closed.
func TestWaitForServiceContextClosedClientIsNotContextError(t *testing.T) {
	closed := openTestClient()
	_ = closed.Close()
	err := closed.WaitForServiceContext(context.Background())
	if err == nil {
		t.Fatal("want an error from a closed client")
	}
	if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("a closed client must not read as a context error: %v", err)
	}
}

// failingReq runs hook inside SerializeCDR and then fails.
type failingReq struct{ hook func() }

func (r *failingReq) TypeName() string { return "x" }
func (r *failingReq) TypeHash() string { return "y" }
func (r *failingReq) SerializeCDR() ([]byte, error) {
	r.hook()
	return nil, errSerializerFailed
}
func (r *failingReq) DeserializeCDR([]byte) error { return nil }

// A serializer error that follows the end of ctx returns the context error.
func TestCallTypedContextEndedDuringFailingSerializeReturnsCtxErr(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	err := CallTypedContext(ctx, openTestClient(), &failingReq{hook: cancel}, &MockMessage{})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
}

// Positive control: with a live context the serializer error comes back as it is.
func TestCallTypedContextFailingSerializeReturnsSerializerError(t *testing.T) {
	err := CallTypedContext(context.Background(), openTestClient(), &failingReq{hook: func() {}}, &MockMessage{})
	if !errors.Is(err, errSerializerFailed) {
		t.Fatalf("want the serializer error, got %v", err)
	}
}

// The service client behind the call ends every query after 10 seconds, so a later deadline
// must not raise the timeout that reaches the C layer. A saturated duration, which time.Until
// returns for a deadline more than 292 years away, must not overflow on the way.
func TestCallTypedContextCapsTimeoutAtQueryLimit(t *testing.T) {
	cases := []struct {
		name string
		dl   time.Time
	}{
		{"an hour away", time.Now().Add(time.Hour)},
		{"saturated", time.Unix(1<<40, 0)},
	}
	for _, tc := range cases {
		var got atomic.Uint64
		stubServiceCall(t, func(_ []byte, timeoutMs uint64) ([]byte, int32) {
			got.Store(timeoutMs)
			return []byte{7}, 0
		})
		ctx, cancel := context.WithDeadline(context.Background(), tc.dl)
		err := CallTypedContext(ctx, openTestClient(), mockReq(), &MockMessage{})
		cancel()
		if err != nil {
			t.Fatalf("%s: unexpected error: %v", tc.name, err)
		}
		if ms := got.Load(); ms != 10000 {
			t.Errorf("%s: timeout passed to C = %d ms, want 10000", tc.name, ms)
		}
	}
}
