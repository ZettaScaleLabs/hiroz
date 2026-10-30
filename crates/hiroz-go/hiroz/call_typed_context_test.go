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
	req := &countingMessage{}
	err := CallTypedContext(ctx, openTestClient(), req, &MockMessage{})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if req.serialised != 0 {
		t.Errorf("SerializeCDR ran %d times for a cancelled call", req.serialised)
	}
}

type countingMessage struct {
	MockMessage
	serialised int
}

func (m *countingMessage) SerializeCDR() ([]byte, error) {
	m.serialised++
	return []byte{1}, nil
}
