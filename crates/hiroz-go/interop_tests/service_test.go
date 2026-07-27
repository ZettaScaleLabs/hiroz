//go:build integration
// +build integration

package interop_tests

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/hiroz"
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

	// Create hiroz-go service server connected to the test router
	hirozCtx, err := hiroz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer hirozCtx.Close()

	node, err := hirozCtx.CreateNode("go_service_server").Build()
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

	// Create hiroz-go service client connected to the test router
	hirozCtx, err := hiroz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer hirozCtx.Close()

	node, err := hirozCtx.CreateNode("go_service_client").Build()
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
	if err := hiroz.CallTyped(client, req, &resp); err != nil {
		t.Fatalf("Service call failed: %v", err)
	}

	if resp.Sum != 17 {
		t.Errorf("Expected sum=17, got sum=%d", resp.Sum)
	}
}

// TestGoServiceServerToGoClient tests Go service server with Go client.
// This tests hiroz to hiroz service communication without ROS2 involvement.
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
	serverCtx, err := hiroz.NewContext().
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
	clientCtx, err := hiroz.NewContext().
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
		if err := hiroz.CallTyped(client, req, &resp); err != nil {
			t.Errorf("Service call failed for %d + %d: %v", tc.a, tc.b, err)
			continue
		}

		if resp.Sum != tc.expected {
			t.Errorf("Expected %d + %d = %d, got %d",
				tc.a, tc.b, tc.expected, resp.Sum)
		}
	}

}

// TestGoServiceClientTimeout verifies that a service call which never gets a
// reply surfaces a distinct ErrorCodeServiceTimeout, not a generic failure.
//
// This is the end-to-end regression for issue #220: the FFI
// `hiroz_service_client_call` previously returned a generic -1 on timeout, so
// the Go binding's ErrorCodeServiceTimeout branch was dead code — a caller
// could not tell a timeout apart from any other call failure.
//
// Test flow:
// - Start a Zenoh router (no service server is ever created)
// - Create a Go service client for a service that has no server
// - Call it with a short timeout
// - Assert the returned error wraps ErrorCodeServiceTimeout
func TestGoServiceClientTimeout(t *testing.T) {
	router := startZenohRouter(t)

	hirozCtx, err := hiroz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer hirozCtx.Close()

	node, err := hirozCtx.CreateNode("go_timeout_client").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	svc := &example_interfaces.AddTwoInts{}
	client, err := node.CreateServiceClient("no_such_server").Build(svc)
	if err != nil {
		t.Fatalf("Failed to create service client: %v", err)
	}
	defer client.Close()

	// No server exists on this key, so the call must time out.
	req := &example_interfaces.AddTwoIntsRequest{A: 1, B: 2}
	var resp example_interfaces.AddTwoIntsResponse
	err = hiroz.CallTypedWithTimeout(client, req, &resp, 300*time.Millisecond)
	if err == nil {
		t.Fatal("expected a timeout error, got nil")
	}

	var herr hiroz.HirozError
	if !errors.As(err, &herr) {
		t.Fatalf("expected a HirozError, got %T: %v", err, err)
	}
	if herr.Code() != hiroz.ErrorCodeServiceTimeout {
		t.Errorf("expected ErrorCodeServiceTimeout (%d), got %d: %v",
			hiroz.ErrorCodeServiceTimeout, herr.Code(), err)
	}
	if !herr.Timeout() {
		t.Errorf("expected Timeout() == true, got false: %v", err)
	}
	if !errors.Is(err, hiroz.ErrTimeout) {
		t.Errorf("expected errors.Is(err, ErrTimeout) == true: %v", err)
	}
}

// TestServiceWithCustomTypes tests service with custom message types.
// This would test services using custom-defined service types
// to ensure the code generation and FFI work correctly for
// user-defined service types, not just standard messages.
func TestServiceWithCustomTypes(t *testing.T) {
	t.Skip("Requires custom service type code generation")
}

// TestGoServiceReplyCorrelationAfterTimeout is a deterministic regression for
// the reply-correlation bug (issue #241): the FFI service client took the next
// reply off a shared channel without matching it to the request that was sent,
// so a late reply from a timed-out call was mis-delivered to the following call.
//
// It forces exactly that race: the server stalls the first request past the
// client's timeout, so call #1 times out and its reply lands late in the shared
// channel. Call #2, with different arguments, must return its OWN reply — not
// the stale one. Pre-fix this returned call #1's result; post-fix the client
// discards the non-matching reply and waits for its own.
func TestGoServiceReplyCorrelationAfterTimeout(t *testing.T) {
	router := startZenohRouter(t)

	hirozCtx, err := hiroz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer hirozCtx.Close()

	node, err := hirozCtx.CreateNode("go_reply_correlation").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	svc := &example_interfaces.AddTwoInts{}

	// Use a service name unique to this test. The interop environment runs many
	// nodes — other tests in this file, and ROS 2 nodes — that expose the
	// conventional "add_two_ints" service on the shared domain. If this test
	// used that name, one of those other servers could answer call #1's query
	// immediately, so call #1 would get a reply (Sum=2) instead of timing out —
	// the actual cause of this test's historical flakiness. A distinct name
	// guarantees this test's server is the ONLY responder, so call #1's outcome
	// is controlled entirely by the release gate below.
	const svcName = "reply_corr_add_two_ints"

	// req1's reply is gated on an explicit release, not a fixed-duration stall.
	// The server blocks the {A:1} request until the test releases it, which
	// happens only AFTER call #1 has already timed out — so no reply can exist
	// during call #1's window and it times out deterministically (a fixed
	// time.Sleep stall would instead depend on scheduling: call #1's timeout is a
	// recv_timeout on the reply channel, and a starved timeout thread could wake
	// only after a stalled reply had already landed and return it). req2 ({A:7})
	// is never gated and replies immediately.
	release := make(chan struct{})
	var releaseOnce sync.Once
	// releaseReply is idempotent and deferred, so the server goroutine blocked on
	// <-release can never wedge the test (and server.Close()) if an assertion
	// fails before the explicit release below.
	releaseReply := func() { releaseOnce.Do(func() { close(release) }) }
	defer releaseReply()

	server, err := node.CreateServiceServer(svcName).
		Build(svc, func(reqBytes []byte) ([]byte, error) {
			var req example_interfaces.AddTwoIntsRequest
			if err := req.DeserializeCDR(reqBytes); err != nil {
				return nil, err
			}
			if req.A == 1 {
				<-release
			}
			resp := &example_interfaces.AddTwoIntsResponse{Sum: req.A + req.B}
			return resp.SerializeCDR()
		})
	if err != nil {
		t.Fatalf("Failed to create service server: %v", err)
	}
	defer server.Close()

	client, err := node.CreateServiceClient(svcName).Build(svc)
	if err != nil {
		t.Fatalf("Failed to create service client: %v", err)
	}
	defer client.Close()

	if err := client.WaitForService(20 * time.Second); err != nil {
		t.Fatalf("service not ready: %v", err)
	}

	// Call #1: this test's server (the only responder for svcName) is blocked on
	// <-release and cannot reply, so this times out no matter how the runner
	// schedules threads.
	req1 := &example_interfaces.AddTwoIntsRequest{A: 1, B: 1} // would be 2
	var resp1 example_interfaces.AddTwoIntsResponse
	if err := hiroz.CallTypedWithTimeout(client, req1, &resp1, 500*time.Millisecond); err == nil {
		t.Fatalf("call #1 expected a timeout, but it returned Sum=%d", resp1.Sum)
	}

	// Release req1's reply now, after call #1 has timed out, so it arrives late.
	// Give it time to reach the client and (on the pre-fix shared-channel bug)
	// sit where call #2 would wrongly pick it up.
	releaseReply()
	time.Sleep(200 * time.Millisecond)

	// Call #2: distinct args, generous timeout. Must get its own reply (7+8=15),
	// not the stale reply from call #1 (2).
	req2 := &example_interfaces.AddTwoIntsRequest{A: 7, B: 8} // 15
	var resp2 example_interfaces.AddTwoIntsResponse
	if err := hiroz.CallTypedWithTimeout(client, req2, &resp2, 5*time.Second); err != nil {
		t.Fatalf("call #2 failed: %v", err)
	}
	if resp2.Sum != 15 {
		t.Fatalf("reply mis-correlated: call #2 (7+8) got Sum=%d, want 15 "+
			"(stale reply from the timed-out call #1 was delivered instead)", resp2.Sum)
	}
}
