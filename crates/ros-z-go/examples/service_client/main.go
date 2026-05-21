// crates/ros-z-go/examples/service_client/main.go
//
// This example demonstrates how to call a ROS 2 service using ros-z Go bindings.
// It calls an AddTwoInts service and prints the result.
//
// Prerequisites:
// 1. Run `just codegen` to generate the message types
// 2. Build the Rust library with `just build-rust`
// 3. Start the service server first (service_server example or ROS 2 equivalent)
//
// Run this example with:
//
//	CGO_LDFLAGS="-L../../../target/release" go run main.go
package main

import (
	"log"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

func main() {
	log.Println("Starting ros-z Go service client example...")

	// Create a ROS 2 context
	ctx, err := rosz.NewContext().
		WithDomainID(0).
		Build()
	if err != nil {
		log.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	// Create a node
	node, err := ctx.CreateNode("go_add_two_ints_client").Build()
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

	// Wait for the service server to appear in the graph before calling.
	if err := client.WaitForService(10 * time.Second); err != nil {
		log.Fatalf("Service not available: %v", err)
	}

	// Call the service with typed request/response.
	req := &example_interfaces.AddTwoIntsRequest{A: 5, B: 3}
	resp := &example_interfaces.AddTwoIntsResponse{}
	log.Printf("Sending request: %d + %d", req.A, req.B)

	if err := rosz.CallTyped(client, req, resp); err != nil {
		log.Fatalf("Service call failed: %v", err)
	}

	log.Printf("Response: %d + %d = %d", req.A, req.B, resp.Sum)
}
