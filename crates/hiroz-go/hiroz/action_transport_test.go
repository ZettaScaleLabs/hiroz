package hiroz

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"testing"
	"time"
)

// These sessions never discover or connect to an existing router. In particular,
// an empty listen list alone does not clear the ROS default localhost:7447 peer.
func actionTransportNodes(t *testing.T, topology string) (*Node, *Node) {
	t.Helper()
	newContext := func(mode ZenohMode, listen, connect []string) *Context {
		t.Helper()
		configuration, err := json.Marshal(map[string]any{
			"listen/endpoints":           listen,
			"connect/endpoints":          connect,
			"scouting/multicast/enabled": false,
			"scouting/gossip/enabled":    false,
			"scouting/delay":             0,
		})
		if err != nil {
			t.Fatal(err)
		}
		ctx, err := NewContext().WithMode(mode).WithJSON(string(configuration)).Build()
		if err != nil {
			t.Fatalf("build %s session: %v", mode, err)
		}
		actionTransportCleanup(t, "context", ctx.Close)
		return ctx
	}
	newNode := func(ctx *Context, name string) *Node {
		t.Helper()
		node, err := ctx.CreateNode(name).Build()
		if err != nil {
			t.Fatal(err)
		}
		actionTransportCleanup(t, "node", node.Close)
		return node
	}
	empty := []string{}
	var serverContext, clientContext *Context
	switch topology {
	case "local":
		serverContext = newContext(ModePeer, empty, empty)
		clientContext = serverContext
	case "peer", "router":
		listener, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		endpoint := []string{"tcp/" + listener.Addr().String()}
		if err := listener.Close(); err != nil {
			t.Fatal(err)
		}
		if topology == "peer" {
			serverContext = newContext(ModePeer, endpoint, empty)
			clientContext = newContext(ModePeer, empty, endpoint)
		} else {
			newContext(ModeRouter, endpoint, empty)
			serverContext = newContext(ModeClient, empty, endpoint)
			clientContext = newContext(ModeClient, empty, endpoint)
		}
	default:
		t.Fatalf("unknown topology %q", topology)
	}
	return newNode(serverContext, "action_transport_server"), newNode(clientContext, "action_transport_client")
}

func actionTransportCleanup(t *testing.T, name string, close func() error) {
	t.Helper()
	t.Cleanup(func() {
		done := make(chan error, 1)
		go func() { done <- close() }()
		select {
		case err := <-done:
			if err != nil {
				t.Errorf("close %s: %v", name, err)
			}
		case <-time.After(5 * time.Second):
			t.Errorf("close %s exceeded five seconds", name)
		}
	})
}

// Keep the successful requests outstanding beyond the old ten-second transport
// deadline while other goals cancel and another wait abandons its local request.
// The same workload runs over an isolated session, peer TCP and router TCP.
func TestActionTransportConcurrentLifecycle(t *testing.T) {
	for _, topology := range []string{"local", "peer", "router"} {
		t.Run(topology, func(t *testing.T) {
			t.Parallel()
			serverNode, clientNode := actionTransportNodes(t, topology)
			ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
			defer cancel()
			release := make(chan struct{})
			server, err := serverNode.CreateActionServer("/action_transport").
				WithResultTimeout(time.Minute).
				WithCancelCallback(func(*ServerGoalHandle) bool { return true }).
				Build(&fixtureAction{}, func([]byte) bool { return true }, func(h *ServerGoalHandle, data []byte) ([]byte, error) {
					goal := &actionFixture{}
					if err := goal.DeserializeCDR(data); err != nil {
						return nil, err
					}
					if err := h.PublishFeedback(&actionFixture{Tag: 2, Code: goal.Code, Value: float64(goal.Code) + 0.5}); err != nil {
						return nil, err
					}
					result := &actionFixture{Tag: 3, Code: goal.Code, Value: float64(goal.Code) + 1000}
					ticker := time.NewTicker(10 * time.Millisecond)
					defer ticker.Stop()
					for {
						if h.IsCancelRequested() {
							return nil, h.Canceled(result)
						}
						select {
						case <-release:
							return result.SerializeCDR()
						case <-h.Context().Done():
							return nil, h.Context().Err()
						case <-ticker.C:
						}
					}
				})
			if err != nil {
				t.Fatal(err)
			}
			actionTransportCleanup(t, "server", func() error {
				if err := server.Close(); err != nil {
					return err
				}
				wait, stop := context.WithTimeout(context.Background(), 3*time.Second)
				defer stop()
				return server.WaitClosed(wait)
			})
			client, err := clientNode.CreateActionClient("/action_transport").WithRequestTimeout(3 * time.Second).Build(&fixtureAction{})
			if err != nil {
				t.Fatal(err)
			}
			actionTransportCleanup(t, "client", client.Close)
			ready, stopReady := context.WithTimeout(ctx, 5*time.Second)
			defer stopReady()
			if err := client.WaitForServer(ready); err != nil {
				t.Fatalf("discover action server: %v", err)
			}
			if err := server.WaitForFeedbackSubscriber(ready); err != nil {
				t.Fatalf("match feedback subscriber: %v", err)
			}

			const goalCount, waitersPerGoal = 4, 2
			type sentGoal struct {
				index  int
				handle *GoalHandle
				err    error
			}
			sent := make(chan sentGoal, goalCount)
			for i := range goalCount {
				go func() {
					h, err := client.SendGoalWithContext(ctx, &actionFixture{Tag: 1, Code: uint16(i)})
					sent <- sentGoal{i, h, err}
				}()
			}
			goals := make([]*GoalHandle, goalCount)
			for range goalCount {
				select {
				case reply := <-sent:
					if reply.err != nil {
						t.Fatalf("send goal %d: %v", reply.index, reply.err)
					}
					goals[reply.index] = reply.handle
					actionTransportCleanup(t, fmt.Sprintf("goal %d", reply.index), reply.handle.Close)
				case <-ctx.Done():
					t.Fatal(ctx.Err())
				}
			}
			for i, goal := range goals {
				feedback := &actionFixture{}
				wait, stop := context.WithTimeout(ctx, 3*time.Second)
				err := ReceiveTypedFeedback(wait, goal, feedback)
				stop()
				if err != nil || *feedback != (actionFixture{Tag: 2, Code: uint16(i), Value: float64(i) + 0.5}) {
					t.Fatalf("goal %d feedback=%+v error=%v", i, feedback, err)
				}
			}

			type resultReply struct {
				index   int
				status  GoalStatus
				data    []byte
				elapsed time.Duration
				err     error
			}
			results := make(chan resultReply, goalCount*waitersPerGoal)
			started := make(chan struct{}, goalCount*waitersPerGoal)
			for i, goal := range goals {
				for range waitersPerGoal {
					go func() {
						begin := time.Now()
						started <- struct{}{}
						status, data, err := goal.GetResultWithStatus(ctx)
						results <- resultReply{i, status, data, time.Since(begin), err}
					}()
				}
			}
			for range goalCount * waitersPerGoal {
				<-started
			}
			finish := time.AfterFunc(12*time.Second, func() { close(release) })
			defer finish.Stop()

			short, stopShort := context.WithTimeout(ctx, 150*time.Millisecond)
			_, _, err = goals[0].GetResultWithStatus(short)
			stopShort()
			if !errors.Is(err, context.DeadlineExceeded) {
				t.Fatalf("short result wait: got %v, want context deadline", err)
			}
			for _, i := range []int{1, 3} {
				response, err := goals[i].CancelWithContext(ctx)
				if err != nil || response.ReturnCode != CancelAccepted || len(response.GoalsCanceling) != 1 || response.GoalsCanceling[0].GoalID != goals[i].GoalID() {
					t.Fatalf("cancel goal %d: response=%+v error=%v", i, response, err)
				}
			}

			checkResult := func(index int, status GoalStatus, data []byte, err error) {
				t.Helper()
				wantStatus := GoalStatusSucceeded
				if index%2 == 1 {
					wantStatus = GoalStatusCanceled
				}
				result := &actionFixture{}
				if err == nil {
					err = result.DeserializeCDR(data)
				}
				want := actionFixture{Tag: 3, Code: uint16(index), Value: float64(index) + 1000}
				if err != nil || status != wantStatus || *result != want {
					t.Fatalf("goal %d result: status=%v payload=%+v error=%v; want %v %+v", index, status, result, err, wantStatus, want)
				}
			}
			for range goalCount * waitersPerGoal {
				select {
				case reply := <-results:
					checkResult(reply.index, reply.status, reply.data, reply.err)
					if reply.index%2 == 0 && reply.elapsed < 12*time.Second {
						t.Fatalf("goal %d result completed after %v, want at least 12 seconds", reply.index, reply.elapsed)
					}
					if reply.index%2 == 1 && reply.elapsed > 5*time.Second {
						t.Fatalf("canceled goal %d took %v to return its result", reply.index, reply.elapsed)
					}
				case <-ctx.Done():
					t.Fatal(ctx.Err())
				}
			}
			// Result retrieval remains repeatable, including the goal whose earlier
			// local wait timed out. Neither wait cancellation nor retrieval consumes it.
			for i, goal := range goals {
				for range 2 {
					wait, stop := context.WithTimeout(ctx, 2*time.Second)
					status, data, err := goal.GetResultWithStatus(wait)
					stop()
					checkResult(i, status, data, err)
				}
				if goal.DroppedFeedback() != 0 {
					t.Errorf("goal %d dropped feedback: %d", i, goal.DroppedFeedback())
				}
			}
		})
	}
}
