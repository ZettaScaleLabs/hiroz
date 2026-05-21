//go:build integration
// +build integration

package interop_tests

import (
	"context"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

// TestGoServiceServerToROS2Client tests Go service server with ROS2 client.
//
// Test flow:
// - Start Zenoh router
// - Create Go node and service server for AddTwoInts
// - Register callback that adds a + b
// - Call service from ROS2: ros2 service call /add_two_ints example_interfaces/srv/AddTwoInts "{a: 5, b: 3}"
// - Verify response: sum=8
func TestGoServiceServerToROS2Client(t *testing.T) {
	if !checkROS2Available() {
		t.Skip("ROS2 not available")
	}

	router := startZenohRouter(t)

	// Create ros-z-go service server connected to the test router
	roszCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer roszCtx.Close()

	node, err := roszCtx.CreateNode("go_service_server").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create service server
	svc := &example_interfaces.AddTwoInts{}
	server, err := node.CreateServiceServer("add_two_ints").
		Build(svc, func(reqBytes []byte) ([]byte, error) {
			var req example_interfaces.AddTwoIntsRequest
			if err := req.DeserializeCDR(reqBytes); err != nil {
				return nil, err
			}

			resp := &example_interfaces.AddTwoIntsResponse{
				Sum: req.A + req.B,
			}

			return resp.SerializeCDR()
		})
	if err != nil {
		t.Fatalf("Failed to create service server: %v", err)
	}
	defer server.Close()

	// Verify service is ready by waiting on the ROS graph before invoking ROS2 CLI.
	selfClient, err := node.CreateServiceClient("add_two_ints").Build(svc)
	if err != nil {
		t.Fatalf("Failed to create self-check client: %v", err)
	}
	defer selfClient.Close()
	if err := selfClient.WaitForService(20 * time.Second); err != nil {
		t.Fatalf("Go service server not ready: %v", err)
	}

	// Call service from ROS2
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "ros2", "service", "call",
		"/add_two_ints",
		"example_interfaces/srv/AddTwoInts",
		"{a: 5, b: 3}")
	cmd.Env = append(os.Environ(), getROS2Env(router)...)

	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("Failed to call service from ROS2: %v\nOutput: %s", err, output)
	}

	// Verify response
	outputStr := string(output)
	if !strings.Contains(outputStr, "sum=8") && !strings.Contains(outputStr, "sum: 8") {
		t.Errorf("Expected sum=8 in response, got: %s", outputStr)
	}
}

// TestROS2ServiceServerToGoClient tests ROS2 service server with Go client.
//
// Test flow:
// - Start Zenoh router
// - Start ROS2 service server: ros2 run demo_nodes_cpp add_two_ints_server
// - Create Go service client
// - Call service with request {a: 10, b: 7}
// - Verify response: sum=17
func TestROS2ServiceServerToGoClient(t *testing.T) {
	if !checkROS2Available() {
		t.Skip("ROS2 not available")
	}

	router := startZenohRouter(t)

	// Start ROS2 service server
	serverCmd := exec.Command("ros2", "run",
		"demo_nodes_cpp", "add_two_ints_server")
	serverCmd.Env = append(os.Environ(), getROS2Env(router)...)
	if err := serverCmd.Start(); err != nil {
		t.Fatalf("Failed to start ROS2 service server: %v", err)
	}
	defer func() {
		serverCmd.Process.Kill()
		serverCmd.Wait()
	}()

	// Wait for ROS2 server to be ready
	time.Sleep(2 * time.Second)

	// Create ros-z-go service client connected to the test router
	roszCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer roszCtx.Close()

	node, err := roszCtx.CreateNode("go_service_client").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create service client
	svc := &example_interfaces.AddTwoInts{}
	client, err := node.CreateServiceClient("add_two_ints").Build(svc)
	if err != nil {
		t.Fatalf("Failed to create service client: %v", err)
	}
	defer client.Close()

	// Wait for discovery
	time.Sleep(500 * time.Millisecond)

	// Call service
	req := &example_interfaces.AddTwoIntsRequest{A: 10, B: 7}
	var resp example_interfaces.AddTwoIntsResponse
	if err := rosz.CallTyped(client, req, &resp); err != nil {
		t.Fatalf("Service call failed: %v", err)
	}

	if resp.Sum != 17 {
		t.Errorf("Expected sum=17, got sum=%d", resp.Sum)
	}
}

// TestGoServiceServerToGoClient tests Go service server with Go client.
// This tests ros-z to ros-z service communication without ROS2 involvement.
//
// Test flow:
// - Start Zenoh router
// - Create Go service server on add_two_ints
// - Create Go service client for add_two_ints
// - Call service multiple times
// - Verify all responses are correct
func TestGoServiceServerToGoClient(t *testing.T) {
	router := startZenohRouter(t)

	// Create server context and node
	serverCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create server context: %v", err)
	}
	defer serverCtx.Close()

	serverNode, err := serverCtx.CreateNode("go_server").Build()
	if err != nil {
		t.Fatalf("Failed to create server node: %v", err)
	}
	defer serverNode.Close()

	// Create service server
	svc := &example_interfaces.AddTwoInts{}
	server, err := serverNode.CreateServiceServer("add_two_ints").
		Build(svc, func(reqBytes []byte) ([]byte, error) {
			var req example_interfaces.AddTwoIntsRequest
			if err := req.DeserializeCDR(reqBytes); err != nil {
				return nil, err
			}

			resp := &example_interfaces.AddTwoIntsResponse{
				Sum: req.A + req.B,
			}

			return resp.SerializeCDR()
		})
	if err != nil {
		t.Fatalf("Failed to create service server: %v", err)
	}
	defer server.Close()

	// Create client context and node
	clientCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create client context: %v", err)
	}
	defer clientCtx.Close()

	clientNode, err := clientCtx.CreateNode("go_client").Build()
	if err != nil {
		t.Fatalf("Failed to create client node: %v", err)
	}
	defer clientNode.Close()

	// Create service client
	client, err := clientNode.CreateServiceClient("add_two_ints").Build(svc)
	if err != nil {
		t.Fatalf("Failed to create service client: %v", err)
	}
	defer client.Close()

	// Wait for discovery
	time.Sleep(300 * time.Millisecond)

	// Test multiple service calls
	testCases := []struct {
		a, b, expected int64
	}{
		{1, 2, 3},
		{10, 20, 30},
		{-5, 15, 10},
		{100, 200, 300},
	}

	for _, tc := range testCases {
		req := &example_interfaces.AddTwoIntsRequest{A: tc.a, B: tc.b}
		var resp example_interfaces.AddTwoIntsResponse
		if err := rosz.CallTyped(client, req, &resp); err != nil {
			t.Errorf("Service call failed for %d + %d: %v", tc.a, tc.b, err)
			continue
		}

		if resp.Sum != tc.expected {
			t.Errorf("Expected %d + %d = %d, got %d",
				tc.a, tc.b, tc.expected, resp.Sum)
		}
	}

}

// TestServiceWithCustomTypes tests service with custom message types.
// This would test services using custom-defined service types
// to ensure the code generation and FFI work correctly for
// user-defined service types, not just standard messages.
func TestServiceWithCustomTypes(t *testing.T) {
	t.Skip("Requires custom service type code generation")
}
