// crates/ros-z-go/examples/action_server/main.go
//
// This example demonstrates how to create a ROS 2 action server using ros-z Go bindings.
// It creates a Fibonacci action server that computes Fibonacci sequences.
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
	"log"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

func main() {
	log.Println("Starting ros-z Go action server example...")

	// Create a ROS 2 context
	ctx, err := rosz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	// Create a node
	node, err := ctx.CreateNode("go_fibonacci_action_server").Build()
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create a typed action server — no manual SerializeCDR/DeserializeCDR needed.
	server, err := rosz.BuildTypedActionServer(
		node.CreateActionServer("fibonacci"),
		&example_interfaces.Fibonacci{},
		// Goal callback: accept all goals with order > 0
		func(goal *example_interfaces.FibonacciGoal) bool {
			log.Printf("Received goal request: order=%d", goal.Order)
			return goal.Order > 0
		},
		// Execute callback: compute Fibonacci sequence with feedback
		func(handle *rosz.ServerGoalHandle, goal *example_interfaces.FibonacciGoal) (*example_interfaces.FibonacciResult, error) {
			log.Printf("Executing goal: computing Fibonacci(%d)", goal.Order)

			sequence := []int32{0, 1}
			for i := 2; i < int(goal.Order); i++ {
				sequence = append(sequence, sequence[i-1]+sequence[i-2])

				// Publish feedback via the goal handle
				if err := handle.PublishFeedback(&example_interfaces.FibonacciFeedback{
					Sequence: sequence,
				}); err != nil {
					log.Printf("Warning: failed to publish feedback: %v", err)
				}

				time.Sleep(500 * time.Millisecond)
			}

			log.Printf("Goal complete: sequence=%v", sequence)
			return &example_interfaces.FibonacciResult{Sequence: sequence}, nil
		},
	)
	if err != nil {
		log.Fatalf("Failed to create action server: %v", err)
	}
	defer server.Close()
	log.Println("Action server 'fibonacci' ready")

	log.Println("Waiting for goals... Press Ctrl+C to exit")

	// Wait for interrupt signal
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)
	<-sigChan

	log.Println("Shutting down...")
}
