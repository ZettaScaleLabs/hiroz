package hiroz

import "testing"

const rejectedBuilds = 1000

// A build that fails must not keep the handler or the memory it captured.
// Each case builds many subscribers and compares the live callback state before and after.
func TestRejectedSubscriberBuildReleasesCallbackState(t *testing.T) {
	cases := []struct {
		name  string
		group BufferGroup
	}{
		{"invalid group", BufferGroup(3)},
		{"negative group", BufferGroup(-1)},
		// The node has no session, so the C creator returns null for a valid group.
		{"creation failure", BufferGroupCpu},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			before := liveClosures.Load()
			for i := 0; i < rejectedBuilds; i++ {
				captured := make([]byte, 1024)
				sub, err := (&Node{}).CreateSubscriber("chatter").
					WithBufferGroup(tc.group).
					BuildWithCallback(&MockMessage{typeName: "std_msgs/msg/String", typeHash: "RIHS01_build_failure"}, func(data []byte) { _ = captured })
				if err == nil || sub != nil {
					t.Fatalf("build %d: want an error and no subscriber, got sub=%v err=%v", i, sub, err)
				}
			}
			if leaked := liveClosures.Load() - before; leaked != 0 {
				t.Errorf("%d rejected builds left %d callback handles alive, want 0", rejectedBuilds, leaked)
			}
		})
	}
}
