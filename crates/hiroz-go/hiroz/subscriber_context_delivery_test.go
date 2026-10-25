package hiroz

import (
	"context"
	"strings"
	"testing"
	"time"
)

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

func ctxDeliveryFixture(t *testing.T, topic string) (*Node, *Publisher) {
	t.Helper()
	ctx, err := NewContext().DisableMulticastScouting().Build()
	if err != nil {
		t.Fatalf("context: %v", err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode("ctx_delivery_fixture").Build()
	if err != nil {
		t.Fatalf("node: %v", err)
	}
	t.Cleanup(func() { node.Close() })
	pub, err := node.CreatePublisher(topic).Build(&ctxDeliveryMsg{})
	if err != nil {
		t.Fatalf("publisher: %v", err)
	}
	t.Cleanup(func() { pub.Close() })
	return node, pub
}

// ctxAwait publishes on a ticker until a value arrives on got or 8 s pass. The subscriber may
// not be matched yet when the first message goes out, so the publisher repeats.
func ctxAwait[V any](t *testing.T, pub *Publisher, got <-chan V) (V, bool) {
	t.Helper()
	deadline := time.After(8 * time.Second)
	tick := time.NewTicker(100 * time.Millisecond)
	defer tick.Stop()
	for {
		if err := pub.Publish(&ctxDeliveryMsg{Value: "hello"}); err != nil {
			t.Fatalf("publish: %v", err)
		}
		select {
		case v := <-got:
			return v, true
		case <-deadline:
			var zero V
			return zero, false
		case <-tick.C:
		}
	}
}

// Positive control: the raw callback decodes into a non-nil value and receives the message.
func TestCtxDeliveryRawCallbackControl(t *testing.T) {
	node, pub := ctxDeliveryFixture(t, "/ctx_delivery_control")
	got := make(chan string, 64)
	sub, err := node.CreateSubscriber("/ctx_delivery_control").BuildWithCallback(&ctxDeliveryMsg{}, func(data []byte) {
		m := &ctxDeliveryMsg{}
		if m.DeserializeCDR(data) == nil {
			got <- m.Value
		}
	})
	if err != nil {
		t.Fatalf("subscriber: %v", err)
	}
	defer sub.Close()
	if _, ok := ctxAwait(t, pub, got); !ok {
		t.Fatal("control: no message received")
	}
}

func TestSubscriberWithChannelContextDeliversForPointerType(t *testing.T) {
	node, pub := ctxDeliveryFixture(t, "/ctx_delivery_chan")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	_, ch, err := SubscriberWithChannelContext[*ctxDeliveryMsg](ctx, node.CreateSubscriber("/ctx_delivery_chan"), 16)
	if err != nil {
		t.Fatalf("subscriber: %v", err)
	}
	m, ok := ctxAwait(t, pub, ch)
	if !ok {
		t.Fatal("no message received")
	}
	if m == nil || m.Value != "hello" {
		t.Fatalf("got %+v, want Value hello", m)
	}
}

func TestBuildWithTypedCallbackContextDeliversForPointerType(t *testing.T) {
	node, pub := ctxDeliveryFixture(t, "/ctx_delivery_cb")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	got := make(chan string, 64)
	_, err := BuildWithTypedCallbackContext(ctx, node.CreateSubscriber("/ctx_delivery_cb"), func(m *ctxDeliveryMsg) { got <- m.Value })
	if err != nil {
		t.Fatalf("subscriber: %v", err)
	}
	v, ok := ctxAwait(t, pub, got)
	if !ok {
		t.Fatal("no message received")
	}
	if v != "hello" {
		t.Fatalf("got %q, want hello", v)
	}
}
