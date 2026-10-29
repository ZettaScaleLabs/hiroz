package hiroz

import "testing"

// The typed server helpers decode into a value of the callback's type. A pointer type such
// as *receiverMsg must decode into an allocated pointee, not a nil receiver.

func TestTypedGoalCallbackDecodesPointerType(t *testing.T) {
	var got string
	cb := typedGoalCallback(func(g *receiverMsg) bool { got = g.Value; return true })
	if !cb([]byte("goal")) || got != "goal" {
		t.Fatalf("accepted/got = %q, want goal accepted", got)
	}
}

func TestTypedExecuteCallbackDecodesPointerType(t *testing.T) {
	cb := typedExecuteCallback(func(_ *ServerGoalHandle, g *receiverMsg) (*receiverMsg, error) {
		return &receiverMsg{Value: g.Value + "!"}, nil
	})
	out, err := cb(nil, []byte("goal"))
	if err != nil || string(out) != "goal!" {
		t.Fatalf("out %q err %v, want goal!", out, err)
	}
}

func TestTypedServiceCallbackDecodesPointerType(t *testing.T) {
	cb := typedServiceCallback(func(r *receiverMsg) (*receiverMsg, error) {
		return &receiverMsg{Value: r.Value + "?"}, nil
	})
	out, err := cb([]byte("req"))
	if err != nil || string(out) != "req?" {
		t.Fatalf("out %q err %v, want req?", out, err)
	}
}
