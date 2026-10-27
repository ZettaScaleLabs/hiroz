package hiroz

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func topicSchema(t *testing.T) *MessageSchema {
	t.Helper()
	s, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/TopicAudit", Fields: []FieldDescription{
		{Name: "label", Type: FieldTypeDescription{ID: FieldString}},
		{Name: "counter", Type: FieldTypeDescription{ID: FieldUint64}},
		{Name: "values", Type: FieldTypeDescription{ID: FieldInt16 + 144}},
	}}})
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func TestDynamicNativeCDR(t *testing.T) {
	schema := topicSchema(t)
	message := schema.NewMessage()
	message.Values = map[string]any{"label": "hello", "counter": uint64(math.MaxUint64), "values": []int16{-32768, 17, 32767}}
	data, err := message.SerializeCDR()
	if err != nil {
		t.Fatal(err)
	}
	decoded := schema.NewMessage()
	if err = decoded.DeserializeCDR(data); err != nil {
		t.Fatal(err)
	}
	if decoded.Values["counter"].(json.Number).String() != "18446744073709551615" {
		t.Fatalf("integer truncated: %#v", decoded.Values)
	}
	if decoded.Values["label"] != "hello" {
		t.Fatalf("bad decoded values: %#v", decoded.Values)
	}
	again, err := decoded.SerializeCDR()
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, again) {
		t.Fatal("CDR round trip changed bytes")
	}
	for _, values := range []map[string]any{{"counter": -1}, {"values": []int{40000}}, {"unknown": 1}} {
		message.Values = values
		if _, err = message.SerializeCDR(); err == nil {
			t.Fatalf("invalid values accepted: %#v", values)
		}
	}
	if err = decoded.DeserializeCDR([]byte{0, 1, 0, 0, 255}); err == nil {
		t.Fatal("truncated CDR accepted")
	}
}

func TestDynamicNestedAndBounds(t *testing.T) {
	nested := IndividualTypeDescription{Name: "test_msgs/msg/Child", Fields: []FieldDescription{{Name: "flag", Type: FieldTypeDescription{ID: FieldBool}}}}
	schema, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/Parent", Fields: []FieldDescription{
		{Name: "children", Type: FieldTypeDescription{ID: FieldNested + 48, Capacity: 2, NestedType: nested.Name}},
		{Name: "text", Type: FieldTypeDescription{ID: FieldBoundedString, StringCapacity: 3}},
	}}, Referenced: []IndividualTypeDescription{nested}})
	if err != nil {
		t.Fatal(err)
	}
	message := schema.NewMessage()
	message.Values = map[string]any{"children": []any{map[string]any{"flag": true}, map[string]any{"flag": false}}, "text": "abc"}
	data, err := message.SerializeCDR()
	if err != nil {
		t.Fatal(err)
	}
	if err = schema.NewMessage().DeserializeCDR(data); err != nil {
		t.Fatal(err)
	}
	message.Values["text"] = "abcd"
	if _, err = message.SerializeCDR(); err == nil {
		t.Fatal("bounded string accepted overflow")
	}
	message.Values["text"] = "ok"
	message.Values["children"] = []any{}
	if _, err = message.SerializeCDR(); err == nil {
		t.Fatal("fixed array accepted wrong size")
	}
}

func TestTimestampCacheAndReceiver(t *testing.T) {
	c, _ := NewSampleCache(2)
	at := time.Unix(10, 0)
	payload := []byte{1}
	c.Add(at.Add(time.Second), Sample{Data: payload})
	payload[0] = 9
	c.Add(at, Sample{Data: []byte{2}})
	c.Add(at.Add(2*time.Second), Sample{Data: []byte{3}})
	if c.Len() != 2 {
		t.Fatal("capacity not enforced")
	}
	s, ok := c.BeforeOrAt(at.Add(time.Second))
	if !ok || s.Sample.Data[0] != 1 {
		t.Fatal("timestamp indexing/copy failed")
	}
	s.Sample.Data[0] = 8
	s, _ = c.AfterOrAt(at)
	if s.Sample.Data[0] != 1 {
		t.Fatal("cache exposes mutable storage")
	}
	if len(c.Between(at, at.Add(2*time.Second))) != 2 {
		t.Fatal("interval query failed")
	}
	r := &SampleReceiver{capacity: 1, notify: make(chan struct{}, 1), done: make(chan struct{})}
	r.add(Sample{Data: []byte{1}})
	r.add(Sample{Data: []byte{2}})
	r.stop()
	r.stop()
	if r.Dropped() != 1 {
		t.Fatal("missing dropped count")
	}
	sample, err := r.Receive(context.Background())
	if err != nil || sample.Data[0] != 2 {
		t.Fatal("queue did not retain newest sample")
	}
	if _, err = r.Receive(context.Background()); !errors.Is(err, ErrReceiverClosed) {
		t.Fatalf("expected receiver close: %v", err)
	}
}

var topicTestCounter atomic.Uint64

func topicTestNode(t *testing.T) (*Context, *Node) {
	t.Helper()
	ctx, err := NewContext().WithDomainID(231).DisableMulticastScouting().WithJSON(`{"listen/endpoints":[],"scouting/gossip/enabled":false}`).Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode(fmt.Sprintf("topic_api_%d_%d", os.Getpid(), topicTestCounter.Add(1))).WithNamespace("/topic_audit").WithTypeDescriptionService().Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { node.Close() })
	return ctx, node
}

// This test traverses actual Go -> C -> Rust -> Zenoh -> Go sample delivery,
// graph discovery, schema service, and shutdown; no transport stubs are used.
func TestTopicAPITransport(t *testing.T) {
	ctx, node := topicTestNode(t)
	schema := topicSchema(t)
	receiver, err := node.CreateSubscriber("samples").WithLocality(LocalitySessionLocal).BuildSampleReceiver(schema.NewMessage(), 8)
	if err != nil {
		t.Fatal(err)
	}
	defer receiver.Close()
	publisher, err := node.CreatePublisher("samples").WithEncoding("application/json").Build(schema.NewMessage())
	if err != nil {
		t.Fatal(err)
	}
	defer publisher.Close()
	if err = publisher.WaitForSubscription(3 * time.Second); err != nil {
		t.Fatal(err)
	}
	if err = receiver.Subscriber.WaitForPublisher(3 * time.Second); err != nil {
		t.Fatal(err)
	}
	info, err := receiver.Subscriber.Info()
	if err != nil || info.Name != "/topic_audit/samples" || info.GID == nil {
		t.Fatalf("subscriber info: %#v %v", info, err)
	}
	endpoints, err := ctx.GetEndpointsByNode(EndpointSubscription, info.Node.Name, "/topic_audit")
	if err != nil || len(endpoints) != 1 {
		t.Fatalf("subscriber not in graph: %#v %v", endpoints, err)
	}
	discovered, err := node.DiscoverTopicSchema("samples", 3*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if discovered.TypeHash() != schema.TypeHash() {
		t.Fatal("schema hash changed")
	}
	wait, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if err = publisher.PublishSerialized([]byte(`{"value":42}`)); err != nil {
		t.Fatal(err)
	}
	sample, err := receiver.Receive(wait)
	if err != nil {
		t.Fatal(err)
	}
	if sample.Encoding != "application/json" || sample.SequenceNumber == nil || sample.PublisherGID == nil || sample.SourceTimestamp == nil || sample.Attachment == nil {
		t.Fatalf("lost sample metadata: %#v", sample)
	}
	original := sample
	forward, err := node.CreatePublisher("forwarded").Build(schema.NewMessage())
	if err != nil {
		t.Fatal(err)
	}
	defer forward.Close()
	forwarded, err := node.CreateSubscriber("forwarded").BuildSampleReceiver(schema.NewMessage(), 4)
	if err != nil {
		t.Fatal(err)
	}
	defer forwarded.Close()
	sample.Encoding = "text/plain"
	if err = forward.PublishSample(sample); err != nil {
		t.Fatal(err)
	}
	sample, err = forwarded.Receive(wait)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(sample.Attachment, original.Attachment) || sample.Encoding != "text/plain" || !bytes.Equal(sample.Data, original.Data) {
		t.Fatal("republish did not preserve sample")
	}
	remote, err := node.CreateSubscriber("samples").WithLocality(LocalityRemote).BuildSampleReceiver(schema.NewMessage(), 2)
	if err != nil {
		t.Fatal(err)
	}
	defer remote.Close()
	if err = publisher.PublishSerialized([]byte{7}); err != nil {
		t.Fatal(err)
	}
	short, stop := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer stop()
	if _, err = remote.Receive(short); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("session-local publication reached remote subscriber: %v", err)
	}
	if err = receiver.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err = receiver.Subscriber.Info(); err == nil {
		t.Fatal("closed subscriber still inspectable")
	}
}

func TestTopicContextAndNodeCloseDuringInspection(t *testing.T) {
	ctx, node := topicTestNode(t)
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < 100; i++ {
			_, _ = ctx.GetEndpoints(EndpointPublisher, "/topic_audit/no_topic")
			_, _ = ctx.GetNodeNames()
			_, _ = ctx.GetActionNamesAndTypes()
		}
	}()
	if err := ctx.Close(); err != nil {
		t.Fatal(err)
	}
	<-done
	if _, err := ctx.GetEndpoints(EndpointPublisher, "/topic_audit/no_topic"); err == nil {
		t.Fatal("closed context still accepted graph query")
	}
	if err := node.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := node.CreatePublisher("closed").Build(topicSchema(t).NewMessage()); err == nil {
		t.Fatal("closed node accepted build")
	}
}

func TestTopicSharedMemoryAndDiscoveryEvents(t *testing.T) {
	ctx, node := topicTestNode(t)
	schema := topicSchema(t)
	wait, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	events, err := ctx.WatchEndpoints(wait, EndpointSubscription, "/topic_audit/shm", 5*time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	initial := <-events
	if !initial.Initial || len(initial.Endpoints) != 0 {
		t.Fatalf("bad initial event: %#v", initial)
	}
	receiver, err := node.CreateSubscriber("shm").BuildSampleReceiver(schema.NewMessage(), 4)
	if err != nil {
		t.Fatal(err)
	}
	defer receiver.Close()
	select {
	case event := <-events:
		if event.Err != nil || len(event.Endpoints) != 1 {
			t.Fatalf("missing match: %#v", event)
		}
	case <-wait.Done():
		t.Fatal(wait.Err())
	}
	publisher, err := node.CreatePublisher("shm").WithSharedMemory(1024*1024, 512).Build(schema.NewMessage())
	if err != nil {
		t.Fatal(err)
	}
	defer publisher.Close()
	payload := bytes.Repeat([]byte{0x43}, 128*1024)
	if err = publisher.PublishSerialized(payload); err != nil {
		t.Fatal(err)
	}
	sample, err := receiver.Receive(wait)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(sample.Data, payload) {
		t.Fatal("SHM payload changed")
	}
	if err = receiver.Close(); err != nil {
		t.Fatal(err)
	}
	select {
	case event := <-events:
		if event.Err != nil || len(event.Endpoints) != 0 {
			t.Fatalf("missing unmatch: %#v", event)
		}
	case <-wait.Done():
		t.Fatal(wait.Err())
	}
}

// descriptorOnlyMessage uses the same optional method emitted by codegen while
// deliberately not implementing SchemaMessage.
type descriptorOnlyMessage struct{ message *DynamicMessage }

func (m *descriptorOnlyMessage) TypeName() string              { return m.message.TypeName() }
func (m *descriptorOnlyMessage) TypeHash() string              { return m.message.TypeHash() }
func (m *descriptorOnlyMessage) SerializeCDR() ([]byte, error) { return m.message.SerializeCDR() }
func (m *descriptorOnlyMessage) DeserializeCDR(data []byte) error {
	return m.message.DeserializeCDR(data)
}
func (m *descriptorOnlyMessage) TypeDescriptionJSON() string {
	return string(m.message.Schema.description)
}
func TestGeneratedDescriptorRegistration(t *testing.T) {
	_, node := topicTestNode(t)
	message := &descriptorOnlyMessage{topicSchema(t).NewMessage()}
	publisher, err := node.CreatePublisher("generated_schema").Build(message)
	if err != nil {
		t.Fatal(err)
	}
	defer publisher.Close()
	schema, err := node.DiscoverTopicSchema("generated_schema", 3*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if schema.TypeHash() != message.TypeHash() {
		t.Fatal("generated descriptor registration changed type hash")
	}
}

type schemaService struct{ request, response *MessageSchema }

func (s *schemaService) TypeName() string     { return "test_msgs::srv::dds_::SchemaService_" }
func (s *schemaService) TypeHash() string     { return s.request.TypeHash() }
func (s *schemaService) GetRequest() Message  { return s.request.NewMessage() }
func (s *schemaService) GetResponse() Message { return s.response.NewMessage() }
func TestServiceSchemaDiscovery(t *testing.T) {
	_, node := topicTestNode(t)
	request := topicSchema(t)
	description := request.Description()
	description.Type.Name = "test_msgs/msg/TopicAuditResponse"
	response, err := NewMessageSchema(description)
	if err != nil {
		t.Fatal(err)
	}
	if err = node.RegisterSchema(request); err != nil {
		t.Fatal(err)
	}
	if err = node.RegisterSchema(response); err != nil {
		t.Fatal(err)
	}
	server, err := node.CreateServiceServer("schema_service").Build(&schemaService{request, response}, func(data []byte) ([]byte, error) { return data, nil })
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	a, b, err := node.DiscoverServiceSchemas("schema_service", request.TypeName(), response.TypeName(), 3*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if a.TypeHash() != request.TypeHash() || b.TypeHash() != response.TypeHash() {
		t.Fatal("service schema hash mismatch")
	}
	start := time.Now()
	_, _, err = node.DiscoverServiceSchemas("missing_service", request.TypeName(), response.TypeName(), 20*time.Millisecond)
	if err == nil || time.Since(start) > time.Second {
		t.Fatalf("discovery timeout not bounded: %v", err)
	}
}

func TestDynamicRejectsNonfiniteCDR(t *testing.T) {
	schema, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/Float", Fields: []FieldDescription{{Name: "value", Type: FieldTypeDescription{ID: FieldFloat64}}}}})
	if err != nil {
		t.Fatal(err)
	}
	data := make([]byte, 12)
	data[1] = 1
	binary.LittleEndian.PutUint64(data[4:], math.Float64bits(math.Inf(1)))
	if err = schema.NewMessage().DeserializeCDR(data); err == nil {
		t.Fatal("non-finite CDR float silently converted to JSON null")
	}
}

func TestDynamicDecodeRejectsBoundsAndHostileLengths(t *testing.T) {
	for _, test := range []struct {
		name  string
		field FieldTypeDescription
		wire  []byte
	}{
		{"bounded string", FieldTypeDescription{ID: FieldBoundedString, StringCapacity: 3}, []byte{0, 1, 0, 0, 5, 0, 0, 0, 'a', 'b', 'c', 'd', 0}},
		{"bounded sequence", FieldTypeDescription{ID: FieldInt16 + 96, Capacity: 1}, []byte{0, 1, 0, 0, 2, 0, 0, 0, 1, 0, 2, 0}},
		{"bounded bytes", FieldTypeDescription{ID: FieldUint8 + 96, Capacity: 1}, []byte{0, 1, 0, 0, 2, 0, 0, 0, 1, 2}},
		{"truncated sequence", FieldTypeDescription{ID: FieldInt64 + 144}, []byte{0, 1, 0, 0, 128, 150, 152, 0}}, // ten million elements, no payload
		{"truncated string", FieldTypeDescription{ID: FieldString}, []byte{0, 1, 0, 0, 255, 255, 255, 255}},
	} {
		t.Run(test.name, func(t *testing.T) {
			schema, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/Bounds", Fields: []FieldDescription{{Name: "value", Type: test.field}}}})
			if err != nil {
				t.Fatal(err)
			}
			if err = schema.NewMessage().DeserializeCDR(test.wire); err == nil {
				t.Fatal("invalid wire value accepted")
			}
		})
	}
	// An empty nested message consumes no wire bytes. Its sequence length must
	// still be checked against the allocation budget, not just payload length.
	empty := IndividualTypeDescription{Name: "test_msgs/msg/Empty", Fields: []FieldDescription{}}
	schema, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/EmptySequence", Fields: []FieldDescription{{Name: "values", Type: FieldTypeDescription{ID: FieldNested + 144, NestedType: empty.Name}}}}, Referenced: []IndividualTypeDescription{empty}})
	if err != nil {
		t.Fatal(err)
	}
	if err = schema.NewMessage().DeserializeCDR([]byte{0, 1, 0, 0, 128, 150, 152, 0}); err == nil {
		t.Fatal("oversized zero-wire sequence accepted")
	}
}

func TestDynamicRejectsOversizedFixedDefaults(t *testing.T) {
	for _, capacity := range []uint64{math.MaxUint64, 64 * 1024 * 1024} {
		_, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/Huge", Fields: []FieldDescription{{Name: "data", Type: FieldTypeDescription{ID: FieldUint8 + 48, Capacity: capacity}}}}})
		if err == nil {
			t.Fatalf("unsafe fixed capacity %d accepted", capacity)
		}
	}
	child := IndividualTypeDescription{Name: "test_msgs/msg/LargeChild", Fields: []FieldDescription{{Name: "data", Type: FieldTypeDescription{ID: FieldUint8 + 48, Capacity: 2048}}}}
	_, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/LargeParent", Fields: []FieldDescription{{Name: "children", Type: FieldTypeDescription{ID: FieldNested + 48, Capacity: 2048, NestedType: child.Name}}}}, Referenced: []IndividualTypeDescription{child}})
	if err == nil {
		t.Fatal("oversized recursive fixed defaults accepted")
	}
}

func TestTransientHistoryRejectsRestrictedLocality(t *testing.T) {
	_, node := topicTestNode(t)
	message := topicSchema(t).NewMessage()
	publisher, err := node.CreatePublisher("history_locality").WithQoS(QosTransientLocal()).Build(message)
	if err != nil {
		t.Fatal(err)
	}
	defer publisher.Close()
	if err = publisher.PublishSerialized([]byte("cached local sample")); err != nil {
		t.Fatal(err)
	}
	for _, locality := range []Locality{LocalityRemote, LocalitySessionLocal} {
		receiver, err := node.CreateSubscriber("history_locality").WithQoS(QosTransientLocal()).WithLocality(locality).BuildSampleReceiver(message, 2)
		if err == nil {
			receiver.Close()
			t.Fatal("unsupported history locality accepted")
		}
		if !strings.Contains(err.Error(), "locality") {
			t.Fatalf("unexpected error: %v", err)
		}
	}
	receiver, err := node.CreateSubscriber("history_locality").WithQoS(QosTransientLocal()).BuildSampleReceiver(message, 2)
	if err != nil {
		t.Fatal(err)
	}
	defer receiver.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	sample, err := receiver.Receive(ctx)
	if err != nil || string(sample.Data) != "cached local sample" {
		t.Fatalf("unrestricted history failed: %q %v", sample.Data, err)
	}
}

type legacyTopicMessage struct{ *DynamicMessage }

func (m *legacyTopicMessage) TypeName() string { return "test_msgs/LegacyPayload" }

func TestDiscoveredDynamicSubscriptionPreservesLegacyWireName(t *testing.T) {
	_, node := topicTestNode(t)
	schema, err := NewMessageSchema(TypeDescription{Type: IndividualTypeDescription{Name: "test_msgs/msg/LegacyPayload", Fields: []FieldDescription{{Name: "label", Type: FieldTypeDescription{ID: FieldString}}}}})
	if err != nil {
		t.Fatal(err)
	}
	message := &legacyTopicMessage{schema.NewMessage()}
	message.Values["label"] = "legacy wire payload"
	publisher, err := node.CreatePublisher("legacy_wire").Build(message)
	if err != nil {
		t.Fatal(err)
	}
	defer publisher.Close()
	discovered, err := node.DiscoverTopicSchema("legacy_wire", 3*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if discovered.TypeName() != schema.TypeName() || discovered.Description().Type.Name != schema.TypeName() || discovered.TypeHash() != schema.TypeHash() {
		t.Fatal("discovery changed canonical schema identity")
	}
	if discovered.NewMessage().TypeName() != message.TypeName() {
		t.Fatal("discovery lost legacy wire spelling")
	}
	received := make(chan string, 1)
	decodeErrors := make(chan error, 1)
	subscriber, err := node.CreateSubscriber("legacy_wire").BuildDynamicWithCallback(discovered, func(value *DynamicMessage, err error) {
		if err != nil {
			decodeErrors <- err
			return
		}
		received <- value.Values["label"].(string)
	})
	if err != nil {
		t.Fatal(err)
	}
	defer subscriber.Close()
	if err = publisher.WaitForSubscription(3 * time.Second); err != nil {
		t.Fatal(err)
	}
	if err = publisher.Publish(message); err != nil {
		t.Fatal(err)
	}
	select {
	case value := <-received:
		if value != "legacy wire payload" {
			t.Fatalf("unexpected payload %q", value)
		}
	case err := <-decodeErrors:
		t.Fatal(err)
	case <-time.After(3 * time.Second):
		t.Fatal("dynamic subscriber did not match the legacy publisher wire identity")
	}
	info, err := publisher.Info()
	if err != nil {
		t.Fatal(err)
	}
	if info.TypeName != message.TypeName() || info.TypeHash != message.TypeHash() {
		t.Fatal("publisher wire identity was altered")
	}
}
