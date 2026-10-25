package hiroz

import (
	"runtime"
	"testing"
	"time"
)

// waitUntil yields until cond holds. It fails the test after a generous deadline, which a
// passing run never reaches.
func waitUntil(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		runtime.Gosched()
	}
}
