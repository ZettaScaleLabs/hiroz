// crates/ros-z-go/examples/service_server/main.go
//
// This example demonstrates how to create a ROS 2 service server using ros-z Go bindings.
// It creates an AddTwoInts service server that responds with the sum of two integers.
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
	"fmt"
	"log"
	"os"
	"os/signal"
	"syscall"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

func main() {
	log.Println("Starting ros-z Go service server example...")

	// Create a ROS 2 context
	ctx, err := rosz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	// Create a node
	node, err := ctx.CreateNode("go_add_two_ints_server").Build()
	if err != nil {
		log.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create a service server with callback
	svc := &example_interfaces.AddTwoInts{}
	server, err := node.CreateServiceServer("add_two_ints").
		Build(svc, func(reqBytes []byte) ([]byte, error) {
			// Deserialize request
			var req example_interfaces.AddTwoIntsRequest
			if err := req.DeserializeCDR(reqBytes); err != nil {
				return nil, fmt.Errorf("failed to deserialize request: %w", err)
			}

			sum := req.A + req.B
			log.Printf("Request: %d + %d = %d", req.A, req.B, sum)

			// Serialize response
			resp := &example_interfaces.AddTwoIntsResponse{Sum: sum}
			return resp.SerializeCDR()
		})
	if err != nil {
		log.Fatalf("Failed to create service server: %v", err)
	}
	defer server.Close()
	log.Println("Service server 'add_two_ints' ready")

	log.Println("Waiting for requests... Press Ctrl+C to exit")

	// Wait for interrupt signal
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)
	<-sigChan

	log.Println("Shutting down...")
}
