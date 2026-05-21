// crates/ros-z-go/examples/action_client_errors/main.go
//
// This example demonstrates structured error handling for action clients.
// Shows how to handle goal rejections, result failures, and cancel operations.
//
// Prerequisites:
// 1. Run `just codegen` to generate the message types
// 2. Build the Rust library with `just build-rust`
// 3. Optionally start action_server to see successful interactions
//
// Run this example with:
//
//	CGO_LDFLAGS="-L../../../target/release" go run main.go
package main

import (
	"encoding/binary"
	"errors"
	"log"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

func main() {
	log.Println("Starting ros-z Go action client with error handling example...")

	// Create a ROS 2 context
	ctx, err := rosz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	// Create a node
	node, err := ctx.CreateNode("go_fibonacci_action_client_errors").Build()
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
		// Check if goal was rejected
		if roszErr, ok := err.(rosz.RoszError); ok {
			log.Printf("Goal failed with code %d: %s", roszErr.Code(), roszErr.Message())

			if errors.Is(roszErr, rosz.ErrGoalRejected) {
				log.Println("✗ Goal was rejected by the action server")
				log.Println()
				log.Println("Possible reasons for rejection:")
				log.Println("  - Server is busy processing another goal")
				log.Println("  - Goal parameters are invalid")
				log.Println("  - Server policy doesn't accept this goal")
				log.Println()
				log.Println("Retry strategies:")
				log.Println("  1. Wait and retry with same goal")
				log.Println("  2. Modify goal parameters")
				log.Println("  3. Check server status/policy")
				return
			}

			switch roszErr.Code() {
			case rosz.ErrorCodeActionGoalRejected:
				log.Println("Goal explicitly rejected")
			case rosz.ErrorCodeServiceTimeout:
				log.Println("Goal send timed out (server may be unresponsive)")
			default:
				log.Printf("Unexpected error code: %d", roszErr.Code())
			}
		}

		log.Fatalf("Failed to send goal: %v", err)
	}

	log.Printf("✓ Goal accepted, ID: %x", goalHandle.GoalID())
	log.Printf("  Status: %v", goalHandle.Status())
	log.Printf("  IsActive: %v", goalHandle.IsActive())

	// Get the result
	log.Println("Waiting for result...")
	resultBytes, err := goalHandle.GetResult()
	if err != nil {
		// Check for result retrieval errors
		if roszErr, ok := err.(rosz.RoszError); ok {
			log.Printf("Get result failed with code %d: %s", roszErr.Code(), roszErr.Message())

			switch roszErr.Code() {
			case rosz.ErrorCodeActionResultFailed:
				log.Println("✗ Failed to retrieve action result")
				log.Println("  - Goal may have been aborted")
				log.Println("  - Server may have crashed")
			case rosz.ErrorCodeServiceTimeout:
				log.Println("✗ Result retrieval timed out")
				log.Println("  - Goal may still be executing")
				log.Println("  - Consider increasing timeout")
			default:
				log.Printf("Unexpected error: %v", roszErr)
			}
		}

		log.Fatalf("Failed to get result: %v", err)
	}

	// Deserialize the result
	var result example_interfaces.FibonacciResult
	if err := result.DeserializeCDR(resultBytes); err != nil {
		log.Fatalf("Failed to deserialize result: %v", err)
	}

	log.Printf("✓ Result: sequence=%v", result.Sequence)

	// Demonstrate goal cancellation (if needed)
	log.Println()
	log.Println("Cancellation example:")
	log.Println("  err := goalHandle.Cancel()")
	log.Println("  if roszErr, ok := err.(rosz.RoszError); ok {")
	log.Println("    if roszErr.Code() == rosz.ErrorCodeActionCancelFailed {")
	log.Println("      // Handle cancellation failure")
	log.Println("    }")
	log.Println("  }")

	log.Println()
	log.Println("Error handling patterns demonstrated:")
	log.Println("  ✓ Goal rejection detection with errors.Is(err, rosz.ErrGoalRejected)")
	log.Println("  ✓ Result retrieval error handling")
	log.Println("  ✓ Action-specific error codes")
	log.Println("  ✓ Timeout detection")

	// Suppress unused import warning for binary (used by generated code)
	_ = binary.LittleEndian
}
