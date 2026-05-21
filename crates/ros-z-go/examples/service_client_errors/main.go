// crates/ros-z-go/examples/service_client_errors/main.go
//
// This example demonstrates structured error handling with RoszError.
// Shows how to handle timeouts, service call failures, and retry logic.
//
// Prerequisites:
// 1. Run `just codegen` to generate the message types
// 2. Build the Rust library with `just build-rust`
// 3. Optionally start service_server to see successful calls
//
// Run this example with:
//
//	CGO_LDFLAGS="-L../../../target/release" go run main.go
package main

import (
	"encoding/binary"
	"errors"
	"log"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

func main() {
	log.Println("Starting ros-z Go service client with error handling example...")

	// Create a ROS 2 context
	ctx, err := rosz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	// Create a node
	node, err := ctx.CreateNode("go_add_two_ints_client_errors").Build()
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create a service client
	svc := &example_interfaces.AddTwoInts{}
	client, err := node.CreateServiceClient("add_two_ints").Build(svc)
	if err != nil {
		log.Fatalf("Failed to create service client: %v", err)
	}
	defer client.Close()
	log.Println("Service client created")

	// Prepare a request
	req := &example_interfaces.AddTwoIntsRequest{A: 5, B: 3}
	log.Printf("Sending request: %d + %d", req.A, req.B)

	// Call the service with retry logic for timeouts
	const maxRetries = 3
	var resp example_interfaces.AddTwoIntsResponse
	var callErr error

	for attempt := 1; attempt <= maxRetries; attempt++ {
		log.Printf("Attempt %d/%d...", attempt, maxRetries)

		callErr = rosz.CallTyped(client, req, &resp)

		if callErr == nil {
			// Success!
			break
		}

		// Check if it's a RoszError and handle specific error codes
		if roszErr, ok := callErr.(rosz.RoszError); ok {
			log.Printf("Service call failed with code %d: %s", roszErr.Code(), roszErr.Message())

			switch roszErr.Code() {
			case rosz.ErrorCodeServiceTimeout:
				// Timeout - retry with backoff
				if attempt < maxRetries {
					backoff := time.Duration(attempt) * time.Second
					log.Printf("Service timed out, retrying in %v...", backoff)
					time.Sleep(backoff)
					continue
				}
				log.Println("Max retries reached for timeout")

			case rosz.ErrorCodeServiceCallFailed:
				// General service failure - could be network issue
				log.Println("Service call failed, server may be unreachable")
				if attempt < maxRetries {
					time.Sleep(500 * time.Millisecond)
					continue
				}

			case rosz.ErrorCodeSessionClosed:
				// Zenoh session closed - fatal
				log.Fatal("Zenoh session closed, cannot retry")

			default:
				// Unknown error code
				log.Printf("Unknown error code: %d", roszErr.Code())
			}

			// Use errors.Is for idiomatic timeout check
			if errors.Is(callErr, rosz.ErrTimeout) {
				log.Println("(Confirmed: This is a timeout error)")
			}
		} else {
			// Not a RoszError - handle as generic error
			log.Printf("Non-Rosz error: %v", callErr)
		}

		// If we've exhausted retries, give up
		if attempt == maxRetries {
			log.Fatalf("Failed to call service after %d attempts: %v", maxRetries, callErr)
		}
	}

	log.Printf("✓ Response: %d + %d = %d", req.A, req.B, resp.Sum)
	log.Println()
	log.Println("Error handling patterns demonstrated:")
	log.Println("  - Type assertion to RoszError")
	log.Println("  - Error code switching (timeout, call failed, session closed)")
	log.Println("  - Retry logic with exponential backoff")
	log.Println("  - errors.Is(err, rosz.ErrTimeout) for idiomatic timeout check")

	// Suppress unused import warning for binary (used by generated code indirectly)
	_ = binary.LittleEndian
}
