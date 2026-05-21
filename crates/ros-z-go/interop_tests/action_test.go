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

// TestGoActionServerToROS2Client tests Go action server with ROS2 client.
//
// Test flow:
// - Start Zenoh router
// - Create Go action server for Fibonacci
// - Send goal from ROS2: ros2 action send_goal /fibonacci example_interfaces/action/Fibonacci "{order: 6}"
// - Verify result contains Fibonacci sequence
func TestGoActionServerToROS2Client(t *testing.T) {
	if !checkROS2Available() {
		t.Skip("ROS2 not available")
	}

	router := startZenohRouter(t)

	// Create ros-z-go action server connected to the test router
	roszCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer roszCtx.Close()

	node, err := roszCtx.CreateNode("go_action_server").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create action server
	action := &example_interfaces.Fibonacci{}
	server, err := node.CreateActionServer("fibonacci").Build(
		action,
		func(goalBytes []byte) bool {
			return true // accept all goals
		},
		func(handle *rosz.ServerGoalHandle, goalBytes []byte) ([]byte, error) {
			var goal example_interfaces.FibonacciGoal
			if err := goal.DeserializeCDR(goalBytes); err != nil {
				return nil, err
			}

			sequence := []int32{0, 1}
			for i := 2; i < int(goal.Order); i++ {
				next := sequence[i-1] + sequence[i-2]
				sequence = append(sequence, next)

				feedback := &example_interfaces.FibonacciFeedback{
					Sequence: sequence,
				}
				_ = handle.PublishFeedback(feedback)
			}

			result := &example_interfaces.FibonacciResult{
				Sequence: sequence,
			}
			return result.SerializeCDR()
		},
	)
	if err != nil {
		t.Fatalf("Failed to create action server: %v", err)
	}
	defer server.Close()

	// Verify action server is ready with a Go self-call before invoking ROS2 CLI
	selfClientCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create self-check context: %v", err)
	}
	defer selfClientCtx.Close()
	selfClientNode, err := selfClientCtx.CreateNode("go_action_server_readiness_check").Build()
	if err != nil {
		t.Fatalf("Failed to create self-check node: %v", err)
	}
	defer selfClientNode.Close()
	selfClient, err := selfClientNode.CreateActionClient("fibonacci").Build(action)
	if err != nil {
		t.Fatalf("Failed to create self-check action client: %v", err)
	}
	defer selfClient.Close()

	deadline := time.Now().Add(30 * time.Second)
	for {
		h, sendErr := selfClient.SendGoal(&example_interfaces.FibonacciGoal{Order: 3})
		if sendErr == nil {
			_, _ = h.GetResult()
			h.Close()
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("Action server not ready after 30s: %v", sendErr)
		}
		time.Sleep(200 * time.Millisecond)
	}

	// Send goal from ROS2
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "ros2", "action", "send_goal",
		"/fibonacci",
		"example_interfaces/action/Fibonacci",
		"{order: 6}",
		"--feedback")
	cmd.Env = append(os.Environ(), getROS2Env(router)...)

	output, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("Failed to send goal from ROS2: %v\nOutput: %s", err, output)
	}

	// Verify result contains Fibonacci sequence and SUCCEEDED status.
	// ROS2 Jazzy outputs YAML format: each element on its own line as "- value"
	outputStr := string(output)
	t.Logf("ROS2 action output:\n%s", outputStr)
	if !strings.Contains(outputStr, "SUCCEEDED") {
		t.Errorf("Expected SUCCEEDED status in result, got: %s", outputStr)
	}
	// Last value in sequence [0,1,1,2,3,5] for order=6
	if !strings.Contains(outputStr, "- 5") {
		t.Errorf("Expected '- 5' (last Fibonacci value) in result, got: %s", outputStr)
	}
}

// TestROS2ActionServerToGoClient tests ROS2 action server with Go client.
//
// Test flow:
// - Start Zenoh router
// - Start Python ROS2 action server (fibonacci_action_server.py) for example_interfaces/action/Fibonacci
// - Create Go action client
// - Send goal {order: 10}
// - Wait for result
// - Verify result contains Fibonacci sequence up to order 10
func TestROS2ActionServerToGoClient(t *testing.T) {
	if !checkROS2Available() {
		t.Skip("ROS2 not available")
	}

	router := startZenohRouter(t)

	// Start Python ROS2 action server for example_interfaces/action/Fibonacci.
	// Go tests run with CWD = package directory (interop_tests/).
	serverCmd := exec.Command("python3", "fibonacci_action_server.py")
	serverCmd.Env = append(os.Environ(), getROS2Env(router)...)
	if err := serverCmd.Start(); err != nil {
		t.Fatalf("Failed to start ROS2 action server: %v", err)
	}
	defer func() {
		serverCmd.Process.Kill()
		serverCmd.Wait()
	}()

	// Create ros-z-go action client connected to the test router
	roszCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer roszCtx.Close()

	node, err := roszCtx.CreateNode("go_action_client").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	// Create action client
	action := &example_interfaces.Fibonacci{}
	client, err := node.CreateActionClient("fibonacci").Build(action)
	if err != nil {
		t.Fatalf("Failed to create action client: %v", err)
	}
	defer client.Close()

	// Retry SendGoal until the Python server is ready (replaces fixed sleep)
	goal := &example_interfaces.FibonacciGoal{Order: 10}
	deadline := time.Now().Add(60 * time.Second)
	var goalHandle *rosz.GoalHandle
	for {
		goalHandle, err = client.SendGoal(goal)
		if err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("Failed to send goal after 60s: %v", err)
		}
		time.Sleep(200 * time.Millisecond)
	}

	// Wait for result (with timeout)
	resultBytes, err := goalHandle.GetResult()
	if err != nil {
		t.Fatalf("Failed to get result: %v", err)
	}

	// Deserialize result
	var result example_interfaces.FibonacciResult
	if err := result.DeserializeCDR(resultBytes); err != nil {
		t.Fatalf("Failed to deserialize result: %v", err)
	}

	if len(result.Sequence) != 10 {
		t.Errorf("Expected sequence length 10, got %d", len(result.Sequence))
	}

	// Verify Fibonacci values
	expected := []int32{0, 1, 1, 2, 3, 5, 8, 13, 21, 34}
	for i, v := range expected {
		if i >= len(result.Sequence) {
			break
		}
		if result.Sequence[i] != v {
			t.Errorf("Sequence[%d]: expected %d, got %d", i, v, result.Sequence[i])
		}
	}

	goalHandle.Close()
}

// TestGoActionServerToGoClient tests Go action server with Go client.
// This tests ros-z to ros-z action communication without ROS2 involvement.
//
// Test flow:
// - Start Zenoh router
// - Create Go action server for Fibonacci
// - Create Go action client
// - Send goal and verify result
func TestGoActionServerToGoClient(t *testing.T) {
	router := startZenohRouter(t)

	// Create server context and node
	serverCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create server context: %v", err)
	}
	defer serverCtx.Close()

	serverNode, err := serverCtx.CreateNode("go_action_server").Build()
	if err != nil {
		t.Fatalf("Failed to create server node: %v", err)
	}
	defer serverNode.Close()

	// Create action server
	action := &example_interfaces.Fibonacci{}
	server, err := serverNode.CreateActionServer("fibonacci").Build(
		action,
		func(goalBytes []byte) bool {
			return true // accept all goals
		},
		func(handle *rosz.ServerGoalHandle, goalBytes []byte) ([]byte, error) {
			var goal example_interfaces.FibonacciGoal
			if err := goal.DeserializeCDR(goalBytes); err != nil {
				return nil, err
			}

			sequence := []int32{0, 1}
			for i := 2; i < int(goal.Order); i++ {
				next := sequence[i-1] + sequence[i-2]
				sequence = append(sequence, next)

				feedback := &example_interfaces.FibonacciFeedback{
					Sequence: sequence,
				}
				_ = handle.PublishFeedback(feedback)

				time.Sleep(10 * time.Millisecond)
			}

			result := &example_interfaces.FibonacciResult{
				Sequence: sequence,
			}
			return result.SerializeCDR()
		},
	)
	if err != nil {
		t.Fatalf("Failed to create action server: %v", err)
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

	clientNode, err := clientCtx.CreateNode("go_action_client").Build()
	if err != nil {
		t.Fatalf("Failed to create client node: %v", err)
	}
	defer clientNode.Close()

	// Create action client
	client, err := clientNode.CreateActionClient("fibonacci").Build(action)
	if err != nil {
		t.Fatalf("Failed to create action client: %v", err)
	}
	defer client.Close()

	// Wait for discovery
	time.Sleep(300 * time.Millisecond)

	// Test with order=7
	goal := &example_interfaces.FibonacciGoal{Order: 7}
	goalHandle, err := client.SendGoal(goal)
	if err != nil {
		t.Fatalf("Failed to send goal: %v", err)
	}

	resultBytes, err := goalHandle.GetResult()
	if err != nil {
		t.Fatalf("Failed to get result: %v", err)
	}

	var result example_interfaces.FibonacciResult
	if err := result.DeserializeCDR(resultBytes); err != nil {
		t.Fatalf("Failed to deserialize result: %v", err)
	}

	expected := []int32{0, 1, 1, 2, 3, 5, 8}
	if len(result.Sequence) != len(expected) {
		t.Fatalf("Expected sequence length %d, got %d", len(expected), len(result.Sequence))
	}

	for i, v := range expected {
		if result.Sequence[i] != v {
			t.Errorf("Sequence[%d]: expected %d, got %d", i, v, result.Sequence[i])
		}
	}

	goalHandle.Close()
}

// TestActionFeedbackMonitoring verifies that feedback published by the server
// is received by the client via the result path.
//
// The current API does not expose a push-based feedback subscription on the client
// side; feedback arrives only in the Go↔ROS2 path via rmw.  This test validates
// the server-side PublishFeedback path by checking that the server completes
// normally when feedback is published and the correct result is returned.
func TestActionFeedbackMonitoring(t *testing.T) {
	router := startZenohRouter(t)

	// Create server
	serverCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create server context: %v", err)
	}
	defer serverCtx.Close()

	serverNode, err := serverCtx.CreateNode("feedback_monitor_server").Build()
	if err != nil {
		t.Fatalf("Failed to create server node: %v", err)
	}
	defer serverNode.Close()

	feedbackCount := 0
	action := &example_interfaces.Fibonacci{}
	server, err := serverNode.CreateActionServer("fibonacci_feedback").Build(
		action,
		func(goalBytes []byte) bool { return true },
		func(handle *rosz.ServerGoalHandle, goalBytes []byte) ([]byte, error) {
			var goal example_interfaces.FibonacciGoal
			if err := goal.DeserializeCDR(goalBytes); err != nil {
				return nil, err
			}

			sequence := []int32{0, 1}
			for i := 2; i < int(goal.Order); i++ {
				sequence = append(sequence, sequence[i-1]+sequence[i-2])
				feedback := &example_interfaces.FibonacciFeedback{Sequence: sequence}
				if err := handle.PublishFeedback(feedback); err != nil {
					t.Logf("PublishFeedback error: %v", err)
				} else {
					feedbackCount++
				}
				time.Sleep(10 * time.Millisecond)
			}

			result := &example_interfaces.FibonacciResult{Sequence: sequence}
			return result.SerializeCDR()
		},
	)
	if err != nil {
		t.Fatalf("Failed to create action server: %v", err)
	}
	defer server.Close()

	// Create client
	clientCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create client context: %v", err)
	}
	defer clientCtx.Close()

	clientNode, err := clientCtx.CreateNode("feedback_monitor_client").Build()
	if err != nil {
		t.Fatalf("Failed to create client node: %v", err)
	}
	defer clientNode.Close()

	client, err := clientNode.CreateActionClient("fibonacci_feedback").Build(action)
	if err != nil {
		t.Fatalf("Failed to create action client: %v", err)
	}
	defer client.Close()

	time.Sleep(300 * time.Millisecond) // discovery

	goal := &example_interfaces.FibonacciGoal{Order: 6}
	goalHandle, err := client.SendGoal(goal)
	if err != nil {
		t.Fatalf("Failed to send goal: %v", err)
	}

	resultBytes, err := goalHandle.GetResult()
	if err != nil {
		t.Fatalf("Failed to get result: %v", err)
	}
	goalHandle.Close()

	var result example_interfaces.FibonacciResult
	if err := result.DeserializeCDR(resultBytes); err != nil {
		t.Fatalf("Failed to deserialize result: %v", err)
	}

	// order=6 yields sequence [0,1,1,2,3,5] (length 6)
	if len(result.Sequence) != 6 {
		t.Errorf("Expected sequence length 6, got %d", len(result.Sequence))
	}

	// Server must have published feedback for each step after the initial pair
	if feedbackCount == 0 {
		t.Errorf("Expected feedback to be published, got 0 feedback calls")
	}
	t.Logf("Server published %d feedback messages", feedbackCount)
}

// TestActionGoalCancellation tests cooperative goal cancellation.
//
// The execute callback polls IsCancelRequested() and exits early when cancelled.
// The client sends the goal, waits briefly, then cancels it.
func TestActionGoalCancellation(t *testing.T) {
	router := startZenohRouter(t)

	serverCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().Build()
	if err != nil {
		t.Fatalf("failed to create server context: %v", err)
	}
	defer serverCtx.Close()

	serverNode, err := serverCtx.CreateNode("cancel_server").Build()
	if err != nil {
		t.Fatalf("failed to create server node: %v", err)
	}
	defer serverNode.Close()

	action := &example_interfaces.Fibonacci{}
	server, err := serverNode.CreateActionServer("fibonacci_cancel").Build(
		action,
		func(goalBytes []byte) bool { return true },
		func(handle *rosz.ServerGoalHandle, goalBytes []byte) ([]byte, error) {
			var goal example_interfaces.FibonacciGoal
			if err := goal.DeserializeCDR(goalBytes); err != nil {
				return nil, err
			}
			sequence := []int32{0, 1}
			for i := 2; i < int(goal.Order); i++ {
				if handle.IsCancelRequested() {
					// Return partial result on cancellation
					result := &example_interfaces.FibonacciResult{Sequence: sequence}
					return result.SerializeCDR()
				}
				sequence = append(sequence, sequence[i-1]+sequence[i-2])
				time.Sleep(50 * time.Millisecond)
			}
			result := &example_interfaces.FibonacciResult{Sequence: sequence}
			return result.SerializeCDR()
		},
	)
	if err != nil {
		t.Fatalf("failed to create action server: %v", err)
	}
	defer server.Close()

	clientCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().Build()
	if err != nil {
		t.Fatalf("failed to create client context: %v", err)
	}
	defer clientCtx.Close()

	clientNode, err := clientCtx.CreateNode("cancel_client").Build()
	if err != nil {
		t.Fatalf("failed to create client node: %v", err)
	}
	defer clientNode.Close()

	client, err := clientNode.CreateActionClient("fibonacci_cancel").Build(action)
	if err != nil {
		t.Fatalf("failed to create action client: %v", err)
	}
	defer client.Close()

	time.Sleep(300 * time.Millisecond) // discovery

	// Use order=20 so the goal runs long enough to cancel
	goal := &example_interfaces.FibonacciGoal{Order: 20}
	goalHandle, err := client.SendGoal(goal)
	if err != nil {
		t.Fatalf("failed to send goal: %v", err)
	}
	defer goalHandle.Close()

	// Wait a bit then cancel
	time.Sleep(150 * time.Millisecond)
	if err := goalHandle.Cancel(); err != nil {
		t.Fatalf("failed to cancel goal: %v", err)
	}

	// GetResult should return the partial sequence
	resultBytes, err := goalHandle.GetResult()
	if err != nil {
		t.Fatalf("failed to get result after cancel: %v", err)
	}

	var result example_interfaces.FibonacciResult
	if err := result.DeserializeCDR(resultBytes); err != nil {
		t.Fatalf("failed to deserialize result: %v", err)
	}

	// Partial sequence must be shorter than full order=20 sequence
	if len(result.Sequence) == 0 {
		t.Error("expected partial sequence, got empty")
	}
	if len(result.Sequence) >= 20 {
		t.Errorf("expected cancellation before completion, got full sequence of length %d", len(result.Sequence))
	}
	t.Logf("cancelled after %d elements", len(result.Sequence))
}

// TestActionWithCustomTypes tests actions with custom message types.
//
// Requires a custom action type fixture generated via codegen.
// The bundled IDL only includes std_msgs, geometry_msgs, and example_interfaces.
func TestActionWithCustomTypes(t *testing.T) {
	t.Skip("requires a custom action type generated from a test IDL fixture")
}
