// crates/ros-z-go/examples/action_client/main.go
//
// This example demonstrates how to send goals to a ROS 2 action server using ros-z Go bindings.
// It sends a Fibonacci goal and retrieves the result.
//
// Prerequisites:
// 1. Run `just codegen` to generate the message types
// 2. Build the Rust library with `just build-rust`
// 3. Start the action server first (action_server example or ROS 2 equivalent)
//
// Run this example with:
//
//	CGO_LDFLAGS="-L../../../target/release" go run main.go
package main

import (
	"log"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

func main() {
	log.Println("Starting ros-z Go action client example...")

	// Create a ROS 2 context
	ctx, err := rosz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	// Create a node
	node, err := ctx.CreateNode("go_fibonacci_action_client").Build()
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create an action client
	action := &example_interfaces.Fibonacci{}
	client, err := node.CreateActionClient("fibonacci").Build(action)
	if err != nil {
		log.Fatalf("Failed to create action client: %v", err)
	}
	defer client.Close()
	log.Println("Action client created")

	// Send a goal
	goal := &example_interfaces.FibonacciGoal{Order: 10}
	log.Printf("Sending goal: order=%d", goal.Order)

	goalHandle, err := client.SendGoal(goal)
	if err != nil {
		log.Fatalf("Failed to send goal: %v", err)
	}
	log.Printf("Goal accepted, ID: %x", goalHandle.GoalID())

	// Get the result
	resultBytes, err := goalHandle.GetResult()
	if err != nil {
		log.Fatalf("Failed to get result: %v", err)
	}

	// Deserialize the result
	var result example_interfaces.FibonacciResult
	if err := result.DeserializeCDR(resultBytes); err != nil {
		log.Fatalf("Failed to deserialize result: %v", err)
	}

	log.Printf("Result: sequence=%v", result.Sequence)
}
