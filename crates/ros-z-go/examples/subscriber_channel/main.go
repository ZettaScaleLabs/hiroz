// crates/ros-z-go/examples/subscriber_channel/main.go
//
// This example demonstrates channel-based message delivery by manually
// integrating the Handler interface with callbacks. Shows FIFO and Ring channel patterns.
//
// Note: Direct Handler integration with Subscriber API is future work.
// This example shows the manual pattern using callbacks + channels.
//
// Prerequisites:
// 1. Run `just codegen` to generate the message types
// 2. Build the Rust library with `just build-rust`
//
// Run this example with:
//
//	CGO_LDFLAGS="-L../../../target/release" go run main.go
package main

import (
	"context"
	"log"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/std_msgs"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

func main() {
	log.Println("Starting ros-z Go channel-based subscriber example...")
	log.Println()
	log.Println("This example demonstrates three message delivery patterns:")
	log.Println("  1. FifoChannel - Buffered with backpressure (blocks when full)")
	log.Println("  2. RingChannel - Non-blocking (drops oldest when full)")
	log.Println("  3. Direct callback - Zero allocation (lowest latency)")
	log.Println()

	// Create a ROS 2 context
	ctx, err := rosz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	// Create a node
	node, err := ctx.CreateNode("go_listener_channel").Build()
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Example 1: FifoChannel - manual integration with callback
	log.Println("Creating FIFO channel subscriber (buffer=10)...")
	fifoHandler := rosz.NewFifoChannel[[]byte](10)
	callback, drop, fifoCh := fifoHandler.ToCbDropHandler()

	fifoSub, err := node.CreateSubscriber("fifo_topic").
		BuildWithCallback(&std_msgs.String{}, callback)
	if err != nil {
		log.Fatalf("Failed to create FIFO subscriber: %v", err)
	}
	defer func() {
		fifoSub.Close()
		drop() // Close the channel
	}()

	// Consumer goroutine for FIFO channel
	go func() {
		for data := range fifoCh {
			var msg std_msgs.String
			if err := msg.DeserializeCDR(data); err != nil {
				log.Printf("[FIFO] Deserialize error: %v", err)
				continue
			}
			log.Printf("[FIFO] Received: %s", msg.Data)
			// Simulate slow processing
			time.Sleep(200 * time.Millisecond)
		}
	}()

	// Example 2: RingChannel - drops oldest when full
	log.Println("Creating Ring channel subscriber (capacity=3)...")
	ringHandler := rosz.NewRingChannel[[]byte](3)
	ringCallback, ringDrop, ringCh := ringHandler.ToCbDropHandler()

	ringSub, err := node.CreateSubscriber("ring_topic").
		BuildWithCallback(&std_msgs.String{}, ringCallback)
	if err != nil {
		log.Fatalf("Failed to create Ring subscriber: %v", err)
	}
	defer func() {
		ringSub.Close()
		ringDrop()
	}()

	// Consumer goroutine for Ring channel
	go func() {
		for data := range ringCh {
			var msg std_msgs.String
			if err := msg.DeserializeCDR(data); err != nil {
				log.Printf("[RING] Deserialize error: %v", err)
				continue
			}
			log.Printf("[RING] Received: %s (may have dropped older messages)", msg.Data)
		}
	}()

	// Example 3: Direct callback (for comparison)
	log.Println("Creating direct callback subscriber...")
	directSub, err := node.CreateSubscriber("chatter").
		BuildWithCallback(&std_msgs.String{}, func(data []byte) {
			var msg std_msgs.String
			if err := msg.DeserializeCDR(data); err != nil {
				log.Printf("[DIRECT] Deserialize error: %v", err)
				return
			}
			log.Printf("[DIRECT] Received: %s", msg.Data)
		})
	if err != nil {
		log.Fatalf("Failed to create direct subscriber: %v", err)
	}
	defer directSub.Close()

	log.Println()
	log.Println("All subscribers created. Listening for messages...")
	log.Println("Publish messages to:")
	log.Println("  - /fifo_topic (FIFO buffering)")
	log.Println("  - /ring_topic (ring buffer)")
	log.Println("  - /chatter (direct callback)")
	log.Println()
	log.Println("Press Ctrl+C to exit")

	// Wait for interrupt signal
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	// Create a context for graceful shutdown
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	<-sigChan
	log.Println("Shutting down...")

	// Give goroutines time to finish
	<-shutdownCtx.Done()
}
