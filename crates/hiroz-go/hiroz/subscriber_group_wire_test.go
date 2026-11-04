package hiroz

import (
	"testing"
	"time"
)

// groupWireCase is one subscriber to build. visible says whether the graph must list its topic.
type groupWireCase struct {
	name    string
	group   *BufferGroup
	qos     *QosProfile
	visible bool
}

func groupPtr(g BufferGroup) *BufferGroup { return &g }

func qosPtr(q QosProfile) *QosProfile { return &q }

// topicListed reports whether the graph of ctx lists topic. A subscriber that stays out of the
// buffer group declares no liveliness token, so with no other endpoint its topic is not listed.
// A failed graph query ends the test: an empty result and a failed query must not look alike.
func topicListed(t *testing.T, ctx *Context, topic string) bool {
	t.Helper()
	topics, err := ctx.GetTopicNamesAndTypes()
	if err != nil {
		t.Fatalf("graph query for %s: %v", topic, err)
	}
	for _, info := range topics {
		if info.Name == topic {
			return true
		}
	}
	return false
}

// TestBuiltSubscriberGroupDecidesGraphVisibility builds subscribers through the production
// wrapper and the real C creator. It reads the effect from the graph: a group of Off, or a QoS
// that stays out of the group (transient-local), leaves the topic unlisted. Dropping the group
// or the explicit QoS anywhere between the builder and the C call changes a listing.
//
// The listed cases are the positive control. The test builds every case, listed or not, before
// it waits for the listed ones. A creator declares its token before it returns, so each token
// of the test is declared before the wait starts. The wait shows that this session delivers
// those tokens to the graph. A subscriber that stays out declares no token at all, so an
// unlisted topic is not a pending one. The wait does not prove an order between the delivery of
// two tokens. The graph shows the topic only. It cannot show a QoS other than the
// transient-local exception, so a dropped best-effort profile goes unseen.
func TestBuiltSubscriberGroupDecidesGraphVisibility(t *testing.T) {
	ctx, err := NewContext().DisableMulticastScouting().Build()
	if err != nil {
		t.Fatalf("context: %v", err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode("group_wire").Build()
	if err != nil {
		t.Fatalf("node: %v", err)
	}
	t.Cleanup(func() { node.Close() })

	cases := []groupWireCase{
		{"default", nil, nil, true},
		{"off", groupPtr(BufferGroupOff), nil, false},
		{"cpu", groupPtr(BufferGroupCpu), nil, true},
		{"auto", groupPtr(BufferGroupAuto), nil, true},
		{"off_sensor_qos", groupPtr(BufferGroupOff), qosPtr(QosSensorData()), false},
		{"cpu_sensor_qos", groupPtr(BufferGroupCpu), qosPtr(QosSensorData()), true},
		{"cpu_transient_local", groupPtr(BufferGroupCpu), qosPtr(QosTransientLocal()), false},
		{"auto_transient_local", groupPtr(BufferGroupAuto), qosPtr(QosTransientLocal()), false},
	}
	topics := map[string]string{}
	for _, tc := range cases {
		topic := "/group_wire_" + tc.name
		topics[tc.name] = topic
		b := node.CreateSubscriber(topic)
		if tc.group != nil {
			b.WithBufferGroup(*tc.group)
		}
		if tc.qos != nil {
			b.WithQoS(*tc.qos)
		}
		msg := &MockMessage{typeName: "std_msgs/msg/String", typeHash: "RIHS01_group_wire"}
		if _, err := b.BuildWithCallback(msg, func([]byte) {}); err != nil {
			t.Fatalf("%s: build: %v", tc.name, err)
		}
	}
	// Barrier: every listed topic is in the graph.
	deadline := time.Now().Add(10 * time.Second)
	for {
		var missing []string
		for _, tc := range cases {
			if tc.visible && !topicListed(t, ctx, topics[tc.name]) {
				missing = append(missing, tc.name)
			}
		}
		if len(missing) == 0 {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("positive control: not listed in the graph after 10s: %v", missing)
		}
		time.Sleep(100 * time.Millisecond)
	}
	for _, tc := range cases {
		if got := topicListed(t, ctx, topics[tc.name]); got != tc.visible {
			t.Errorf("%s: listed in the graph = %v, want %v", tc.name, got, tc.visible)
		}
	}
}
