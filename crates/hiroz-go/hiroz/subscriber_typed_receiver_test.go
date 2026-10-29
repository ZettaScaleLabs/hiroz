package hiroz

import (
	"strings"
	"testing"
	"time"
)

// receiverMsg decodes into its receiver, like every generated message type. Calling
// DeserializeCDR on a nil *receiverMsg dereferences nil.
type receiverMsg struct{ Value string }

func (m *receiverMsg) TypeName() string { return "test/ReceiverMsg" }
func (m *receiverMsg) TypeHash() string {
	return "RIHS01_" + strings.Repeat("ab", 32)
}
func (m *receiverMsg) SerializeCDR() ([]byte, error) { return []byte(m.Value), nil }
func (m *receiverMsg) DeserializeCDR(data []byte) error {
	m.Value = string(data)
	return nil
}

func receiverFixture(t *testing.T, topic string) (*Node, *Publisher) {
	t.Helper()
	ctx, err := NewContext().DisableMulticastScouting().Build()
	if err != nil {
		t.Fatalf("context: %v", err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode("receiver_fixture").Build()
	if err != nil {
		t.Fatalf("node: %v", err)
	}
	t.Cleanup(func() { node.Close() })
	pub, err := node.CreatePublisher(topic).Build(&receiverMsg{})
	if err != nil {
		t.Fatalf("publisher: %v", err)
	}
	t.Cleanup(func() { pub.Close() })
	return node, pub
}

// publishUntil publishes on a timer until delivered() holds or 8 s pass.
func publishUntil(t *testing.T, pub *Publisher, delivered func() bool) bool {
	t.Helper()
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		if err := pub.Publish(&receiverMsg{Value: "hello"}); err != nil {
			t.Fatalf("publish: %v", err)
		}
		time.Sleep(100 * time.Millisecond)
		if delivered() {
			return true
		}
	}
	return delivered()
}

// Positive control: the raw callback decodes into a non-nil value and receives the message.
func TestReceiverFixtureRawCallbackDelivers(t *testing.T) {
	node, pub := receiverFixture(t, "/receiver_control")
	got := make(chan string, 64)
	sub, err := node.CreateSubscriber("/receiver_control").BuildWithCallback(&receiverMsg{}, func(data []byte) {
		m := &receiverMsg{}
		if m.DeserializeCDR(data) == nil {
			got <- m.Value
		}
	})
	if err != nil {
		t.Fatalf("subscriber: %v", err)
	}
	defer sub.Close()
	if !publishUntil(t, pub, func() bool { return len(got) > 0 }) {
		t.Fatal("control: no message received")
	}
}

func TestSubscriberWithChannelDeliversForPointerType(t *testing.T) {
	node, pub := receiverFixture(t, "/receiver_chan")
	sub, ch, cleanup, err := SubscriberWithChannel[*receiverMsg](node.CreateSubscriber("/receiver_chan"), 16)
	if err != nil {
		t.Fatalf("subscriber: %v", err)
	}
	defer sub.Close()
	defer cleanup()
	if !publishUntil(t, pub, func() bool { return len(ch) > 0 }) {
		t.Fatal("no message received")
	}
	if m := <-ch; m == nil || m.Value != "hello" {
		t.Fatalf("got %+v, want Value hello", m)
	}
}

func TestBuildWithTypedCallbackDeliversForPointerType(t *testing.T) {
	node, pub := receiverFixture(t, "/receiver_cb")
	got := make(chan string, 64)
	sub, err := BuildWithTypedCallback(node.CreateSubscriber("/receiver_cb"), func(m *receiverMsg) { got <- m.Value })
	if err != nil {
		t.Fatalf("subscriber: %v", err)
	}
	defer sub.Close()
	if !publishUntil(t, pub, func() bool { return len(got) > 0 }) {
		t.Fatal("no message received")
	}
	if v := <-got; v != "hello" {
		t.Fatalf("got %q, want hello", v)
	}
}

func TestSubscriberWithHandlerDeliversForPointerType(t *testing.T) {
	node, pub := receiverFixture(t, "/receiver_handler")
	sub, ch, cleanup, err := SubscriberWithHandler(node.CreateSubscriber("/receiver_handler"), NewFifoChannel[*receiverMsg](16))
	if err != nil {
		t.Fatalf("subscriber: %v", err)
	}
	defer sub.Close()
	defer cleanup()
	if !publishUntil(t, pub, func() bool { return len(ch) > 0 }) {
		t.Fatal("no message received")
	}
	if m := <-ch; m == nil || m.Value != "hello" {
		t.Fatalf("got %+v, want Value hello", m)
	}
}
