package hiroz

import (
	"sync"
	"testing"
	"time"
)

func TestParentCloseDuringGraphReadsAndEndpointBuilds(t *testing.T) {
	node := localCallbackNode(t)
	var wg sync.WaitGroup
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 20; j++ {
				_, _ = node.ctx.GetNodeNames()
				if pub, err := node.CreatePublisher("/parent_close").Build(&callbackMessage{}); err == nil {
					_ = pub.Close()
				}
				if sub, err := node.CreateSubscriber("/parent_close").BuildWithCallback(&callbackMessage{}, func([]byte) {}); err == nil {
					_ = sub.Close()
				}
			}
		}()
	}
	_ = node.Close()
	_ = node.ctx.Close()
	wg.Wait()
	if _, err := node.ctx.GetNodeNames(); err == nil {
		t.Fatal("graph operation succeeded after context closure")
	}
	if _, err := node.CreatePublisher("/after_close").Build(&callbackMessage{}); err == nil {
		t.Fatal("publisher construction succeeded after node closure")
	}
}

func localCallbackNode(t *testing.T) *Node {
	t.Helper()
	ctx, err := NewContext().DisableMulticastScouting().WithJSON(`{"listen/endpoints":[],"scouting/gossip/enabled":false}`).Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode("go_callback_lifecycle").Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { node.Close() })
	return node
}

func TestTypedSubscriberPublicEntryPoints(t *testing.T) {
	node := localCallbackNode(t)
	for _, mode := range []string{"callback", "channel", "handler"} {
		t.Run(mode, func(t *testing.T) {
			topic := "/typed_" + mode
			var sub *Subscriber
			var received <-chan *callbackMessage
			var cleanup func()
			var err error
			switch mode {
			case "callback":
				ch := make(chan *callbackMessage, 1)
				received = ch
				sub, err = BuildWithTypedCallback(node.CreateSubscriber(topic), func(msg *callbackMessage) { ch <- msg })
				cleanup = func() {}
			case "channel":
				sub, received, cleanup, err = SubscriberWithChannel[*callbackMessage](node.CreateSubscriber(topic), 1)
			case "handler":
				sub, received, cleanup, err = SubscriberWithHandler(node.CreateSubscriber(topic), NewFifoChannel[*callbackMessage](1))
			}
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { cleanup(); sub.Close() })
			pub, err := node.CreatePublisher(topic).Build(&callbackMessage{})
			if err != nil {
				t.Fatal(err)
			}
			defer pub.Close()
			if err := pub.Publish(&callbackMessage{Value: 9}); err != nil {
				t.Fatal(err)
			}
			select {
			case msg := <-received:
				if msg == nil || msg.Value != 9 {
					t.Fatalf("received %#v", msg)
				}
			case <-time.After(3 * time.Second):
				t.Fatal("typed callback did not deliver")
			}
			sub.Close()
			cleanup()
			cleanup()
			if mode != "callback" {
				select {
				case _, ok := <-received:
					if ok {
						t.Fatal("channel left open")
					}
				case <-time.After(time.Second):
					t.Fatal("Close did not close output channel")
				}
			}
		})
	}
}

func TestSubscriberCanCloseFromCallback(t *testing.T) {
	node := localCallbackNode(t)
	done := make(chan struct{})
	var sub *Subscriber
	var err error
	sub, err = node.CreateSubscriber("/close_from_callback").BuildWithCallback(&callbackMessage{}, func([]byte) {
		sub.Close()
		close(done)
	})
	if err != nil {
		t.Fatal(err)
	}
	pub, err := node.CreatePublisher("/close_from_callback").Build(&callbackMessage{})
	if err != nil {
		t.Fatal(err)
	}
	defer pub.Close()
	go pub.Publish(&callbackMessage{Value: 1})
	select {
	case <-done:
	case <-time.After(3 * time.Second):
		t.Fatal("subscriber Close deadlocked in its callback")
	}
	node.subsMu.Lock()
	count := len(node.ownedSubs)
	node.subsMu.Unlock()
	if count != 0 {
		t.Fatalf("closed subscriber retained by node: %d", count)
	}
}

func TestPublisherConcurrentPublishAndClose(t *testing.T) {
	node := localCallbackNode(t)
	pub, err := node.CreatePublisher("/publish_close").Build(&callbackMessage{})
	if err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 100; j++ {
				_ = pub.Publish(&callbackMessage{Value: 1})
			}
		}()
	}
	wg.Add(1)
	go func() { defer wg.Done(); pub.Close() }()
	wg.Wait()
	if err := pub.Publish(&callbackMessage{Value: 1}); err == nil {
		t.Fatal("Publish succeeded after Close")
	}
}

// Publish can deliver a local callback synchronously on its caller's thread.
func TestPublisherCanCloseFromLocalSubscriberCallback(t *testing.T) {
	node := localCallbackNode(t)
	pub, err := node.CreatePublisher("/close_publisher_from_callback").Build(&callbackMessage{})
	if err != nil {
		t.Fatal(err)
	}
	defer pub.Close()
	sub, err := node.CreateSubscriber("/close_publisher_from_callback").BuildWithCallback(&callbackMessage{}, func([]byte) {
		pub.Close()
	})
	if err != nil {
		t.Fatal(err)
	}
	defer sub.Close()
	done := make(chan error, 1)
	go func() { done <- pub.Publish(&callbackMessage{Value: 1}) }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("publisher Close deadlocked inside a local subscriber callback")
	}
	if err := pub.Publish(&callbackMessage{Value: 2}); err == nil {
		t.Fatal("publisher remained open")
	}
}
