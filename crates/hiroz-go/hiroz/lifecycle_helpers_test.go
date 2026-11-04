package hiroz

import (
	"runtime"
	"sync"
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

// newRelease returns a channel that a stub waits on and the function that releases it. The
// function is idempotent and also runs at the end of the test, so a failed assertion cannot
// leave the stub goroutine blocked.
func newRelease(t *testing.T) (<-chan struct{}, func()) {
	t.Helper()
	ch := make(chan struct{})
	var once sync.Once
	free := func() { once.Do(func() { close(ch) }) }
	t.Cleanup(free)
	return ch, free
}
