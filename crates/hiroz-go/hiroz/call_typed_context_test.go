package hiroz

import (
	"context"
	"errors"
	"testing"
)

// A cancelled call must not run the caller's serializer.
func TestCallTypedContextCancelledSkipsSerializer(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	serialised := 0
	req := hookMsg(func() { serialised++ })
	err := CallTypedContext(ctx, openTestClient(), req, &MockMessage{})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if serialised != 0 {
		t.Errorf("SerializeCDR ran %d times for a cancelled call", serialised)
	}
}
