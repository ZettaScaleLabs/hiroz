package hiroz

import (
	"context"
	"testing"
	"time"
)

// closeResult runs Close on its own goroutine and reports when it returned.
func closeResult(c *ServiceClient) <-chan struct{} {
	ch := make(chan struct{})
	go func() { _ = c.Close(); close(ch) }()
	return ch
}

// Positive control: Close does wait for a registered C call. The same observation shape as
// the decoder test below therefore detects a Close that waits.
func TestServiceClientCloseWaitsForRegisteredCCall(t *testing.T) {
	c := openTestClient()
	started := make(chan struct{})
	release, free := newRelease(t)
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) {
		close(started)
		<-release
		return nil, 1
	})
	done := make(chan error, 1)
	go func() { done <- CallTypedContext(context.Background(), c, mockReq(), &MockMessage{}) }()
	receiveBounded(t, started, "the C call to start")

	closed := closeResult(c)
	select {
	case <-closed:
		t.Fatal("Close returned while a registered C call was still running")
	case <-time.After(100 * time.Millisecond):
	}
	free()
	receiveBounded(t, closed, "Close to return after the C call ended")
	receiveBounded(t, done, "the call to return")
}

// Close drains registered C operations. It does not wait for the response decoder, which runs
// after the C call has released its registration.
func TestServiceClientCloseDoesNotWaitForDecoder(t *testing.T) {
	c := openTestClient()
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) { return []byte{1}, 0 })
	release, free := newRelease(t)
	entered, exited := make(chan struct{}), make(chan struct{})
	dec := &stubMsg{de: func([]byte) error { close(entered); <-release; close(exited); return nil }}
	done := make(chan error, 1)
	go func() { done <- CallTypedContext(context.Background(), c, mockReq(), dec) }()
	receiveBounded(t, entered, "the decoder to start")

	receiveBounded(t, closeResult(c), "Close to return while the decoder is blocked")
	select {
	case <-exited:
		t.Fatal("the decoder returned before the test released it")
	default:
	}
	free()
	receiveBounded(t, exited, "the decoder to return after release")
	if err := receiveBounded(t, done, "the call to return"); err != nil {
		t.Fatalf("the call returned %v after Close, want nil", err)
	}
}
