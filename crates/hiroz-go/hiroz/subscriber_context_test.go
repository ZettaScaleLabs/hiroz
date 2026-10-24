package hiroz

import (
	"context"
	"errors"
	"testing"
)

func TestSubscriberWithChannelContextAlreadyCancelled(t *testing.T) {
	before := liveClosures.Load()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	sub, ch, err := SubscriberWithChannelContext[*MockTypedMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 4)
	if sub != nil || ch != nil {
		t.Errorf("want no subscriber and no channel, got sub=%v ch=%v", sub, ch)
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if leaked := liveClosures.Load() - before; leaked != 0 {
		t.Errorf("a cancelled context left %d callback handles alive, want 0", leaked)
	}
}

func TestSubscriberWithChannelContextBuildFailure(t *testing.T) {
	// The node has no session, so the C creator returns null.
	sub, ch, err := SubscriberWithChannelContext[*MockTypedMessage](context.Background(), (&Node{}).CreateSubscriber("chatter"), 4)
	if sub != nil || ch != nil {
		t.Errorf("want no subscriber and no channel, got sub=%v ch=%v", sub, ch)
	}
	if !errors.Is(err, ErrBuildFailed) {
		t.Fatalf("want ErrBuildFailed, got %v", err)
	}
}

func TestBuildWithTypedCallbackContextAlreadyCancelled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	sub, err := BuildWithTypedCallbackContext(ctx, (&Node{}).CreateSubscriber("chatter"), func(*MockTypedMessage) {})
	if sub != nil {
		t.Error("a cancelled context must not return a subscriber")
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
}

func TestBuildWithTypedCallbackContextBuildFailure(t *testing.T) {
	sub, err := BuildWithTypedCallbackContext(context.Background(), (&Node{}).CreateSubscriber("chatter"), func(*MockTypedMessage) {})
	if sub != nil || !errors.Is(err, ErrBuildFailed) {
		t.Fatalf("want ErrBuildFailed and no subscriber, got sub=%v err=%v", sub, err)
	}
}
