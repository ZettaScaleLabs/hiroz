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
func topicListed(ctx *Context, topic string) bool {
	topics, err := ctx.GetTopicNamesAndTypes()
	if err != nil {
		return false
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
// The listed cases are the positive control. The test waits until they are all listed before it
// reads the unlisted ones, so an unlisted topic is not a late token. The graph shows the topic
// only. It cannot show a QoS other than the transient-local exception, so a dropped best-effort
// profile goes unseen.
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
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		all := true
		for _, tc := range cases {
			if tc.visible && !topicListed(ctx, topics[tc.name]) {
				all = false
			}
		}
		if all {
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	for _, tc := range cases {
		if got := topicListed(ctx, topics[tc.name]); got != tc.visible {
			t.Errorf("%s: listed in the graph = %v, want %v", tc.name, got, tc.visible)
		}
	}
}
