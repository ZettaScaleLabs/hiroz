package hiroz

import (
	"errors"
	"testing"
)

type callbackMessage struct{ Value byte }

func (m *callbackMessage) TypeName() string { _ = m.Value; return "test/msg/Callback" }
func (m *callbackMessage) TypeHash() string {
	return "RIHS01_0000000000000000000000000000000000000000000000000000000000000000"
}
func (m *callbackMessage) SerializeCDR() ([]byte, error) { return []byte{m.Value}, nil }
func (m *callbackMessage) DeserializeCDR(data []byte) error {
	if len(data) != 1 {
		return errors.New("expected one byte")
	}
	m.Value = data[0]
	return nil
}

func TestTypedSubscriberCallbackAllocatesEachDelivery(t *testing.T) {
	var received []*callbackMessage
	callback, template, err := typedMessageCallback[*callbackMessage](func(msg *callbackMessage) {
		received = append(received, msg)
	})
	if err != nil {
		t.Fatal(err)
	}
	if template == nil {
		t.Fatal("nil type-information template")
	}
	template.TypeName() // methods need not accept a nil receiver
	callback([]byte{1})
	callback(nil)
	callback([]byte{2})
	if len(received) != 2 || received[0].Value != 1 || received[1].Value != 2 || received[0] == received[1] {
		t.Fatalf("deliveries must have distinct populated receivers: %#v", received)
	}
}

func TestTypedServiceCallbackAllocatesRequest(t *testing.T) {
	var received []*callbackMessage
	callback, err := typedServiceCallback(func(req *callbackMessage) (*callbackMessage, error) {
		received = append(received, req)
		return &callbackMessage{Value: req.Value + 1}, nil
	})
	if err != nil {
		t.Fatal(err)
	}
	for _, value := range []byte{2, 4} {
		response, err := callback([]byte{value})
		if err != nil || len(response) != 1 || response[0] != value+1 {
			t.Fatalf("response = %v, error = %v", response, err)
		}
	}
	if _, err := callback(nil); err == nil {
		t.Fatal("accepted malformed request")
	}
	if len(received) != 2 || received[0] == received[1] || received[0].Value != 2 {
		t.Fatalf("requests reused or malformed request delivered: %#v", received)
	}
}

func TestTypedActionCallbacksAllocateGoal(t *testing.T) {
	var accepted, executed *callbackMessage
	accept, execute, err := typedActionCallbacks(
		func(goal *callbackMessage) bool { accepted = goal; return goal.Value == 7 },
		func(_ *ServerGoalHandle, goal *callbackMessage) (*callbackMessage, error) {
			executed = goal
			return &callbackMessage{Value: goal.Value + 1}, nil
		},
	)
	if err != nil {
		t.Fatal(err)
	}
	if accept(nil) {
		t.Fatal("accepted malformed goal")
	}
	if !accept([]byte{7}) {
		t.Fatal("rejected valid goal")
	}
	result, err := execute(nil, []byte{7})
	if err != nil || len(result) != 1 || result[0] != 8 {
		t.Fatalf("result = %v, error = %v", result, err)
	}
	if accepted == nil || executed == nil || accepted == executed {
		t.Fatal("goal receivers must be nonnil and independent")
	}
	if _, err := execute(nil, nil); err == nil {
		t.Fatal("executed malformed goal")
	}
}

func TestTypedCallbacksRejectInterfaceType(t *testing.T) {
	// Rejection occurs before using the builder, so callers get a useful error.
	if _, err := BuildWithTypedCallback[Message](nil, func(Message) {}); err == nil {
		t.Fatal("accepted an interface message type")
	}
	if _, err := BuildTypedServiceServer[Message, Message](nil, nil, func(Message) (Message, error) { return nil, nil }); err == nil {
		t.Fatal("accepted an interface request type")
	}
	if _, err := BuildTypedActionServer[Message, Message](nil, nil, func(Message) bool { return true }, func(*ServerGoalHandle, Message) (Message, error) { return nil, nil }); err == nil {
		t.Fatal("accepted an interface goal type")
	}
}
