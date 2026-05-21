//go:build integration
// +build integration

package interop_tests

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/std_msgs"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

// TestGoPublisherToROS2Subscriber tests ros-z-go publisher -> ROS2 subscriber
func TestGoPublisherToROS2Subscriber(t *testing.T) {
	if !checkROS2Available() {
		t.Skip("ROS2 not available, skipping interop test")
	}

	// Start Zenoh router
	router := startZenohRouter(t)
	defer router.Stop()

	time.Sleep(time.Second) // Wait for router

	// Start ROS2 subscriber in background.
	// --once makes ros2 topic echo exit after receiving the first message.
	// We must Start() it BEFORE creating the publisher so it is already
	// listening when the first message arrives.
	subCtx, subCancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer subCancel()

	cmd := exec.CommandContext(subCtx,
		"ros2", "topic", "echo", "/chatter", "std_msgs/msg/String", "--once")
	cmd.Env = append(os.Environ(), getROS2Env(router)...)

	var outBuf bytes.Buffer
	cmd.Stdout = &outBuf
	cmd.Stderr = &outBuf

	if err := cmd.Start(); err != nil {
		t.Fatalf("Failed to start ROS2 subscriber: %v", err)
	}
	defer func() {
		if cmd.Process != nil {
			cmd.Process.Kill()
		}
		cmd.Wait() //nolint:errcheck
	}()

	// Signal channel closed when the subscriber exits (--once → exits after first message)
	subDone := make(chan error, 1)
	go func() { subDone <- cmd.Wait() }()

	time.Sleep(2 * time.Second) // Wait for subscriber + discovery

	// Create ros-z-go publisher
	roszCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer roszCtx.Close()

	node, err := roszCtx.CreateNode("go_publisher").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create publisher with type information
	msgTemplate := &std_msgs.String{}
	pub, err := node.CreatePublisher("/chatter").Build(msgTemplate)
	if err != nil {
		t.Fatalf("Failed to create publisher: %v", err)
	}
	defer pub.Close()

	time.Sleep(time.Second) // Let discovery happen

	// Publish until the subscriber exits (received --once) or we hit the limit
	for i := 0; i < 10; i++ {
		msg := &std_msgs.String{Data: "Hello from Go!"}
		if err := pub.Publish(msg); err != nil {
			t.Errorf("Publish failed: %v", err)
		}
		t.Logf("Published message %d", i+1)

		select {
		case <-subDone:
			// Subscriber received the message and exited
			goto checkOutput
		default:
		}
		time.Sleep(500 * time.Millisecond)
	}

	// Wait a bit more for the subscriber to finish
	select {
	case <-subDone:
	case <-time.After(5 * time.Second):
	}

checkOutput:
	output := outBuf.String()
	t.Logf("ROS2 subscriber output: %s", output)
	if !strings.Contains(output, "data:") && !strings.Contains(output, "Hello from Go") {
		t.Errorf("ROS2 subscriber did not receive message, output: %s", output)
	}
}

// TestROS2PublisherToGoSubscriber tests ROS2 publisher -> ros-z-go subscriber
func TestROS2PublisherToGoSubscriber(t *testing.T) {
	if !checkROS2Available() {
		t.Skip("ROS2 not available, skipping interop test")
	}

	// Start Zenoh router
	router := startZenohRouter(t)
	defer router.Stop()

	time.Sleep(time.Second)

	// Create ros-z-go subscriber
	roszCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer roszCtx.Close()

	node, err := roszCtx.CreateNode("go_subscriber").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Track received messages
	var mu sync.Mutex
	received := []string{}

	// Create subscriber with callback.
	// Keep the reference alive — if it is discarded the GC may collect the
	// subscriber and destroy the Zenoh subscription before any messages arrive.
	sub, err := node.CreateSubscriber("/chatter").BuildWithCallback(&std_msgs.String{}, func(data []byte) {
		msg := &std_msgs.String{}
		if err := msg.DeserializeCDR(data); err != nil {
			t.Logf("Deserialize warning: %v (raw len=%d)", err, len(data))
			return
		}
		mu.Lock()
		received = append(received, msg.Data)
		t.Logf("Received: %s", msg.Data)
		mu.Unlock()
	})
	if err != nil {
		t.Fatalf("Failed to create subscriber: %v", err)
	}
	defer sub.Close()

	time.Sleep(2 * time.Second) // Wait for subscriber + discovery

	// Start ROS2 publisher.
	// Use -w 0 so ros2 topic pub does not wait for rmw_zenoh_cpp-visible
	// subscribers — the Go subscriber uses ros-z liveliness, not rmw liveliness.
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	cmd := exec.CommandContext(ctx,
		"ros2", "topic", "pub", "/chatter", "std_msgs/msg/String",
		"{data: 'Hello from ROS2'}", "--rate", "2", "--times", "10",
		"-w", "0")
	cmd.Env = append(os.Environ(), getROS2Env(router)...)

	if err := cmd.Start(); err != nil {
		t.Fatalf("Failed to start ROS2 publisher: %v", err)
	}

	// Wait for messages (10 messages at 2 Hz = 5 s + rmw startup overhead)
	time.Sleep(10 * time.Second)

	if cmd.Process != nil {
		cmd.Process.Kill()
		cmd.Wait() //nolint:errcheck
	}

	// Check received messages
	mu.Lock()
	defer mu.Unlock()

	if len(received) == 0 {
		t.Errorf("Expected to receive messages, got none")
	} else {
		t.Logf("Successfully received %d messages from ROS2", len(received))
		for _, msg := range received {
			if !strings.Contains(msg, "Hello from ROS2") {
				t.Errorf("Unexpected message content: %s", msg)
			}
		}
	}
}

// TestGoPublisherToGoSubscriber tests ros-z-go publisher -> ros-z-go subscriber
func TestGoPublisherToGoSubscriber(t *testing.T) {
	// Start Zenoh router
	router := startZenohRouter(t)
	defer router.Stop()

	time.Sleep(time.Second)

	// Create publisher context
	pubCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create publisher context: %v", err)
	}
	defer pubCtx.Close()

	pubNode, err := pubCtx.CreateNode("go_publisher").Build()
	if err != nil {
		t.Fatalf("Failed to create publisher node: %v", err)
	}
	defer pubNode.Close()

	// Create subscriber context
	subCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create subscriber context: %v", err)
	}
	defer subCtx.Close()

	subNode, err := subCtx.CreateNode("go_subscriber").Build()
	if err != nil {
		t.Fatalf("Failed to create subscriber node: %v", err)
	}
	defer subNode.Close()

	// Track received messages
	var mu sync.Mutex
	received := []string{}
	done := make(chan struct{})
	var closeOnce sync.Once

	// Create subscriber
	_, err = subNode.CreateSubscriber("/test_topic").BuildWithCallback(&std_msgs.String{}, func(data []byte) {
		msg := &std_msgs.String{}
		if err := msg.DeserializeCDR(data); err != nil {
			t.Errorf("Failed to deserialize: %v", err)
			return
		}
		mu.Lock()
		received = append(received, msg.Data)
		t.Logf("Received: %s", msg.Data)
		if len(received) >= 3 {
			closeOnce.Do(func() { close(done) })
		}
		mu.Unlock()
	})
	if err != nil {
		t.Fatalf("Failed to create subscriber: %v", err)
	}

	time.Sleep(time.Second) // Wait for discovery

	// Create publisher
	msgTemplate := &std_msgs.String{}
	pub, err := pubNode.CreatePublisher("/test_topic").Build(msgTemplate)
	if err != nil {
		t.Fatalf("Failed to create publisher: %v", err)
	}
	defer pub.Close()

	time.Sleep(time.Second) // Wait for discovery

	// Publish messages
	for i := 0; i < 5; i++ {
		msg := &std_msgs.String{
			Data: "Test message " + string(rune('A'+i)),
		}
		if err := pub.Publish(msg); err != nil {
			t.Errorf("Publish failed: %v", err)
		}
		t.Logf("Published: %s", msg.Data)
		time.Sleep(200 * time.Millisecond)
	}

	// Wait for messages with timeout
	select {
	case <-done:
		t.Logf("Successfully received all messages")
	case <-time.After(10 * time.Second):
		mu.Lock()
		t.Errorf("Timeout waiting for messages, received %d/3", len(received))
		mu.Unlock()
	}

	// Verify we received at least 3 messages
	mu.Lock()
	defer mu.Unlock()
	if len(received) < 3 {
		t.Errorf("Expected at least 3 messages, got %d", len(received))
	}
}
