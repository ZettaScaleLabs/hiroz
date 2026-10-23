package hiroz

import "testing"

// A subscriber built without WithBufferGroup joins the CPU buffer group.
// The check is on the builder alone, so it needs no Zenoh session. The join itself
// is tested on the Rust side, through the C function this option calls.
func TestSubscriberBuilderBufferGroupDefaultsToAuto(t *testing.T) {
	b := (&Node{}).CreateSubscriber("chatter")
	if b.bufferGroup != BufferGroupAuto {
		t.Errorf("default buffer group = %d, want BufferGroupAuto", b.bufferGroup)
	}
}

func TestSubscriberBuilderWithBufferGroup(t *testing.T) {
	b := (&Node{}).CreateSubscriber("chatter")
	if got := b.WithBufferGroup(BufferGroupCpu); got != b {
		t.Fatal("WithBufferGroup must return the same builder")
	}
	if b.bufferGroup != BufferGroupCpu {
		t.Errorf("buffer group = %d, want BufferGroupCpu", b.bufferGroup)
	}
	b.WithBufferGroup(BufferGroupOff)
	if b.bufferGroup != BufferGroupOff {
		t.Errorf("buffer group = %d, want BufferGroupOff", b.bufferGroup)
	}
}

// The values are the C ABI of hiroz_subscriber_create_with_group.
func TestBufferGroupValuesMatchTheCABI(t *testing.T) {
	if BufferGroupOff != 0 {
		t.Errorf("BufferGroupOff = %d, want 0", BufferGroupOff)
	}
	if BufferGroupCpu != 1 {
		t.Errorf("BufferGroupCpu = %d, want 1", BufferGroupCpu)
	}
	if BufferGroupAuto != 2 {
		t.Errorf("BufferGroupAuto = %d, want 2", BufferGroupAuto)
	}
}

// replaceCreateSubscriber swaps the C creator for a recorder that fails every build.
// It returns the groups that reached the C layer, in call order.
func replaceCreateSubscriber(t *testing.T) *[]int32 {
	t.Helper()
	var groups []int32
	original := createSubscriber
	createSubscriber = func(c subscriberCall) *cSubscriber {
		groups = append(groups, c.group)
		return nil
	}
	t.Cleanup(func() { createSubscriber = original })
	return &groups
}

// The group a builder holds is the group the C creator receives.
// The recorder sees the call that BuildWithCallback makes. The join is tested on the Rust side.
func TestBuildWithCallbackSendsTheGroupToC(t *testing.T) {
	cases := []struct {
		name  string
		setup func(b *SubscriberBuilder)
		want  int32
	}{
		{"default", func(b *SubscriberBuilder) {}, 2},
		{"off", func(b *SubscriberBuilder) { b.WithBufferGroup(BufferGroupOff) }, 0},
		{"cpu", func(b *SubscriberBuilder) { b.WithBufferGroup(BufferGroupCpu) }, 1},
		{"auto", func(b *SubscriberBuilder) { b.WithBufferGroup(BufferGroupAuto) }, 2},
		{"last value wins", func(b *SubscriberBuilder) {
			b.WithBufferGroup(BufferGroupCpu)
			b.WithBufferGroup(BufferGroupOff)
		}, 0},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			groups := replaceCreateSubscriber(t)
			b := (&Node{}).CreateSubscriber("chatter")
			tc.setup(b)
			msg := &MockMessage{typeName: "std_msgs/msg/String", typeHash: "RIHS01_group"}
			if _, err := b.BuildWithCallback(msg, func([]byte) {}); err == nil {
				t.Fatal("the recorder returns no handle, so the build must fail")
			}
			if len(*groups) != 1 || (*groups)[0] != tc.want {
				t.Errorf("groups sent to C = %v, want [%d]", *groups, tc.want)
			}
		})
	}
}

// An unknown group stops in Go and never reaches the C creator.
func TestBuildWithCallbackKeepsAnUnknownGroupAwayFromC(t *testing.T) {
	groups := replaceCreateSubscriber(t)
	msg := &MockMessage{typeName: "std_msgs/msg/String", typeHash: "RIHS01_group"}
	_, err := (&Node{}).CreateSubscriber("chatter").WithBufferGroup(BufferGroup(3)).BuildWithCallback(msg, func([]byte) {})
	if err == nil {
		t.Fatal("an unknown group must fail the build")
	}
	if len(*groups) != 0 {
		t.Errorf("the C creator ran %d times for an unknown group, want 0", len(*groups))
	}
}
