package hiroz

import "sync"

// inflightGate counts the calls that use a C handle in a goroutine. A call registers with
// enter before it reads the handle. shutdown refuses new calls first and then waits for the
// registered ones, so no call can register after the wait has started.
type inflightGate struct {
	mu   sync.Mutex
	shut bool
	quit chan struct{}
	wg   sync.WaitGroup
}

// enter registers one call. It returns false once shutdown has started.
func (g *inflightGate) enter() bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.shut {
		return false
	}
	g.wg.Add(1)
	return true
}

// leave ends one call that enter registered.
func (g *inflightGate) leave() { g.wg.Done() }

// quitting returns a channel that closes when shutdown starts.
func (g *inflightGate) quitting() <-chan struct{} {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.quit == nil {
		g.quit = make(chan struct{})
		if g.shut {
			close(g.quit)
		}
	}
	return g.quit
}

// shutdown refuses new calls, wakes the calls that watch quitting, and waits for all calls.
func (g *inflightGate) shutdown() {
	g.mu.Lock()
	if !g.shut {
		g.shut = true
		if g.quit != nil {
			close(g.quit)
		}
	}
	g.mu.Unlock()
	g.wg.Wait()
}
