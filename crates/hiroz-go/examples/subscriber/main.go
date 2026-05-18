// crates/hiroz-go/examples/subscriber/main.go
//
// This example demonstrates how to subscribe to a ROS 2 topic using hiroz Go bindings.
// It listens for std_msgs/String messages on the "chatter" topic.
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
	"log"
	"os"
	"os/signal"
	"syscall"

	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/generated/std_msgs"
	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/hiroz"
)

func main() {
	log.Println("Starting hiroz Go subscriber example...")

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
	node, err := ctx.CreateNode("go_listener").Build()
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()
	log.Println("Node 'go_listener' created")

	// Create a message template for type information
	msgTemplate := &std_msgs.String{}

	// Create a subscriber with a callback handler
	sub, err := node.CreateSubscriber("chatter").
		BuildWithCallback(msgTemplate, func(data []byte) {
			// Deserialize the received CDR data
			msg := &std_msgs.String{}
			if err := msg.DeserializeCDR(data); err != nil {
				log.Printf("Failed to deserialize message: %v", err)
				return
			}
			log.Printf("Received: %s", msg.Data)
		})
	if err != nil {
		log.Fatalf("Failed to create subscriber: %v", err)
	}
	defer sub.Close()
	log.Println("Subscriber created on topic 'chatter'")

	log.Println("Listening for messages... Press Ctrl+C to exit")

	// Wait for interrupt signal
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)
	<-sigChan

	log.Println("Shutting down...")
}
