package hiroz

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"
)

// firstErrContext makes the first successful preflight cancellation check
// observable, without replacing the production request dispatch path.
type firstErrContext struct {
	context.Context
	checked chan struct{}
	once    sync.Once
}

func (c *firstErrContext) Err() error {
	err := c.Context.Err()
	c.once.Do(func() { close(c.checked) })
	return err
}

func TestServiceCanceledBeforeDispatchDoesNotSendRequest(t *testing.T) {
	client, server := serviceTestPair(t, "/cancel_before_dispatch")
	base, cancel := context.WithCancel(context.Background())
	defer cancel()
	ctx := &firstErrContext{Context: base, checked: make(chan struct{})}
	var result <-chan serviceCallResult
	func() {
		// Another start/close operation may hold this mutex while the caller
		// passes its initial context check. Cancellation must be rechecked
		// before transmitting a request when the mutex becomes available.
		client.mu.Lock()
		defer client.mu.Unlock()
		result = asyncServiceCall(client, ctx, []byte{1})
		select {
		case <-ctx.checked:
		case <-time.After(time.Second):
			t.Fatal("call did not perform its preflight check")
		}
		cancel()
	}()
	select {
	case got := <-result:
		if !errors.Is(got.err, context.Canceled) {
			t.Fatalf("error=%v, want cancellation", got.err)
		}
	case <-time.After(time.Second):
		t.Fatal("canceled call did not return")
	}
	request, err := server.TryTakeRequest()
	if err != nil {
		t.Fatal(err)
	}
	if request != nil {
		request.Close()
		t.Fatal("request was sent after cancellation while waiting to dispatch")
	}
}
