package hiroz

import (
	"context"
	"errors"
	"fmt"
	"sync"
)

var ErrReceiverClosed = errors.New("sample receiver is closed")

// SampleReceiver is a bounded FIFO of owned samples. At capacity it evicts the
// oldest sample; Dropped reports the number evicted. Notification is coalesced.
// Multiple goroutines may Receive concurrently; each sample goes to one caller.
type SampleReceiver struct {
	Subscriber *Subscriber
	mu         sync.Mutex
	samples    []Sample
	capacity   int
	dropped    uint64
	closed     bool
	notify     chan struct{}
	done       chan struct{}
}

func (b *SubscriberBuilder) BuildSampleReceiver(msg Message, capacity int) (*SampleReceiver, error) {
	if capacity < 1 {
		return nil, fmt.Errorf("capacity must be positive")
	}
	r := &SampleReceiver{capacity: capacity, notify: make(chan struct{}, 1), done: make(chan struct{})}
	sub, err := b.buildWithSampleCallback(msg, r.add, r.stop)
	if err != nil {
		return nil, err
	}
	r.Subscriber = sub
	return r, nil
}
func (r *SampleReceiver) signal() {
	select {
	case r.notify <- struct{}{}:
	default:
	}
}
func (r *SampleReceiver) add(sample Sample) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return
	}
	if len(r.samples) == r.capacity {
		copy(r.samples, r.samples[1:])
		r.samples[len(r.samples)-1] = sample
		r.dropped++
	} else {
		r.samples = append(r.samples, sample)
	}
	r.signal()
}
func (r *SampleReceiver) stop() {
	r.mu.Lock()
	defer r.mu.Unlock()
	if !r.closed {
		r.closed = true
		close(r.done)
	}
}
func (r *SampleReceiver) Close() error {
	r.stop()
	if r.Subscriber != nil {
		return r.Subscriber.Close()
	}
	return nil
}
func (r *SampleReceiver) Dropped() uint64 { r.mu.Lock(); defer r.mu.Unlock(); return r.dropped }

// Ready emits coalesced wakeups. Call TryReceive until empty after each wakeup.
// The channel is not closed; use Done to observe shutdown.
func (r *SampleReceiver) Ready() <-chan struct{} { return r.notify }
func (r *SampleReceiver) Done() <-chan struct{}  { return r.done }

// TryReceive returns (sample,true,nil), (zero,false,nil) when empty, or
// ErrReceiverClosed after all retained samples have been drained.
func (r *SampleReceiver) TryReceive() (Sample, bool, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if len(r.samples) > 0 {
		s := r.samples[0]
		r.samples[0] = Sample{}
		r.samples = r.samples[1:]
		if len(r.samples) > 0 {
			r.signal()
		}
		return s, true, nil
	}
	if r.closed {
		return Sample{}, false, ErrReceiverClosed
	}
	return Sample{}, false, nil
}
func (r *SampleReceiver) Receive(ctx context.Context) (Sample, error) {
	if ctx == nil {
		return Sample{}, fmt.Errorf("context is required")
	}
	for {
		if err := ctx.Err(); err != nil {
			return Sample{}, err
		}
		s, ok, err := r.TryReceive()
		if ok || err != nil {
			return s, err
		}
		select {
		case <-ctx.Done():
			return Sample{}, ctx.Err()
		case <-r.done:
		case <-r.notify:
		}
	}
}
