package hiroz

import (
	"sync"
	"testing"
	"time"
)

func TestFifoDropUnblocksSendersAndIsIdempotent(t *testing.T) {
	for _, capacity := range []int{0, 1} {
		fifo := NewFifoChannel[int](capacity)
		send, drop, ch := fifo.ToCbDropHandler()
		if capacity > 0 {
			send(0)
		}
		var wg sync.WaitGroup
		for i := 0; i < 32; i++ {
			wg.Add(1)
			go func() { defer wg.Done(); send(1) }()
		}
		for i := 0; i < 4; i++ {
			wg.Add(1)
			go func() { defer wg.Done(); drop() }()
		}
		done := make(chan struct{})
		go func() { wg.Wait(); close(done) }()
		select {
		case <-done:
		case <-time.After(3 * time.Second):
			t.Fatal("drop did not unblock pending sends")
		}
		send(2) // sends after cleanup are harmless
		drop()
		for range ch {
		}
	}
}

func TestRingConcurrentConsumerDoesNotBlockProducer(t *testing.T) {
	ring := NewRingChannel[int](1)
	send, drop, ch := ring.ToCbDropHandler()
	consumed := make(chan struct{})
	go func() {
		for range ch {
		}
		close(consumed)
	}()
	sent := make(chan struct{})
	go func() {
		for i := 0; i < 100000; i++ {
			send(i)
		}
		drop()
		close(sent)
	}()
	select {
	case <-sent:
	case <-time.After(5 * time.Second):
		t.Fatal("ring producer blocked while consumer drained channel")
	}
	<-consumed
	drop()
	send(0)
}
