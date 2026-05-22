// crates/hiroz-go/examples/publisher/main.go
//
// This example demonstrates how to publish to a ROS 2 topic using hiroz Go bindings.
// It publishes std_msgs/String messages to the "chatter" topic at 10 Hz.
//
// Prerequisites:
// 1. Run `just codegen-bundled` to generate the message types
// 2. Build the Rust library with `just build-rust`
//
// Run this example with:
//
//	CGO_LDFLAGS="-L../../../target/release" go run main.go
package main

import (
	"fmt"
	"log"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/generated/std_msgs"
	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/hiroz"
)

func main() {
	log.Println("Starting hiroz Go publisher example...")

	// Create a ROS 2 context (equivalent to rclcpp::init)
	ctx, err := hiroz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()
	log.Println("Context created")

	// Create a node
	node, err := ctx.CreateNode("go_talker").Build()
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()
	log.Println("Node 'go_talker' created")

	// Create a publisher
	pub, err := node.CreatePublisher("chatter").Build(&std_msgs.String{})
	if err != nil {
		log.Fatalf("Failed to create publisher: %v", err)
	}
	defer pub.Close()
	log.Println("Publisher created on topic 'chatter'")

	// Set up signal handling for graceful shutdown
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	// Publish messages at 10 Hz
	count := 0
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

	log.Println("Publishing messages... Press Ctrl+C to exit")

	for {
		select {
		case <-sigChan:
			log.Println("Shutting down...")
			return
		case <-ticker.C:
			msg := &std_msgs.String{
				Data: fmt.Sprintf("Hello from Go! #%d", count),
			}

			if err := pub.Publish(msg); err != nil {
				log.Printf("Failed to publish: %v", err)
			} else {
				log.Printf("Published: %s", msg.Data)
			}

			count++
		}
	}
}
