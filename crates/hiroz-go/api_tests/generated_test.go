//go:build generated_api

package api_tests

import (
	"bytes"
	"context"
	"reflect"
	"testing"
	"time"

	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/generated/std_msgs"
	"github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/hiroz"
)

func generatedNode(t *testing.T) *hiroz.Node {
	t.Helper()
	ctx, err := hiroz.NewContext().DisableMulticastScouting().WithJSON(`{"listen/endpoints":[],"scouting/gossip/enabled":false}`).Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode("generated_go_api").WithTypeDescriptionService().Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { node.Close() })
	return node
}

func TestGeneratedTopicSchemaAndTypedDelivery(t *testing.T) {
	node := generatedNode(t)
	received := make(chan *std_msgs.String, 1)
	sub, err := hiroz.BuildWithTypedCallback(node.CreateSubscriber("/generated_string"), func(m *std_msgs.String) { received <- m })
	if err != nil {
		t.Fatal(err)
	}
	defer sub.Close()
	pub, err := node.CreatePublisher("/generated_string").Build(&std_msgs.String{})
	if err != nil {
		t.Fatal(err)
	}
	defer pub.Close()
	schema, err := node.DiscoverTopicSchema("/generated_string", 2*time.Second)
	if err != nil || schema.TypeHash() != (&std_msgs.String{}).TypeHash() {
		t.Fatalf("generated schema: %v, %v", schema, err)
	}
	if err := pub.Publish(&std_msgs.String{Data: "typed delivery"}); err != nil {
		t.Fatal(err)
	}
	select {
	case message := <-received:
		if message == nil || message.Data != "typed delivery" {
			t.Fatalf("received %#v", message)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("no generated typed delivery")
	}
}

func TestGeneratedComponentSchemas(t *testing.T) {
	node := generatedNode(t)
	for _, message := range []hiroz.TypeDescriptionMessage{
		&example_interfaces.AddTwoIntsRequest{A: 17, B: -3},
		&example_interfaces.AddTwoIntsResponse{Sum: 14},
		&example_interfaces.FibonacciGoal{Order: 5},
		&example_interfaces.FibonacciResult{Sequence: []int32{0, 1, 1, 2, 3}},
		&example_interfaces.FibonacciFeedback{Sequence: []int32{0, 1}},
	} {
		t.Run(message.TypeName(), func(t *testing.T) {
			schema, err := hiroz.NewMessageSchemaJSON(message.TypeDescriptionJSON())
			if err != nil {
				t.Fatal(err)
			}
			if schema.TypeHash() != message.TypeHash() {
				t.Fatalf("schema hash %s differs from payload hash %s", schema.TypeHash(), message.TypeHash())
			}
			topic := "/generated_schema/" + message.TypeName()
			pub, err := node.CreatePublisher(topic).Build(message)
			if err != nil {
				t.Fatal(err)
			}
			defer pub.Close()
			discovered, err := node.DiscoverTopicSchema(topic, 2*time.Second)
			if err != nil {
				t.Fatal(err)
			}
			if discovered.TypeHash() != message.TypeHash() {
				t.Fatalf("discovered hash %s differs from payload hash %s", discovered.TypeHash(), message.TypeHash())
			}
			if discovered.NewMessage().TypeName() != message.TypeName() {
				t.Fatalf("discovery changed transport type %q to %q", message.TypeName(), discovered.NewMessage().TypeName())
			}
			type delivery struct {
				data []byte
				err  error
			}
			received := make(chan delivery, 1)
			sub, err := node.CreateSubscriber(topic).BuildDynamicWithCallback(discovered, func(decoded *hiroz.DynamicMessage, err error) {
				var data []byte
				if err == nil {
					data, err = decoded.SerializeCDR()
				}
				received <- delivery{data, err}
			})
			if err != nil {
				t.Fatal(err)
			}
			defer sub.Close()
			expected, err := message.SerializeCDR()
			if err != nil {
				t.Fatal(err)
			}
			if err := pub.Publish(message); err != nil {
				t.Fatal(err)
			}
			select {
			case actual := <-received:
				if actual.err != nil || !bytes.Equal(actual.data, expected) {
					t.Fatalf("dynamic delivery=%x, err=%v; want %x", actual.data, actual.err, expected)
				}
			case <-time.After(2 * time.Second):
				t.Fatal("no dynamic delivery for generated component")
			}
		})
	}
}

func TestGeneratedTypedService(t *testing.T) {
	node := generatedNode(t)
	server, err := hiroz.BuildTypedServiceServer(node.CreateServiceServer("/generated_add"), &example_interfaces.AddTwoInts{},
		func(request *example_interfaces.AddTwoIntsRequest) (*example_interfaces.AddTwoIntsResponse, error) {
			return &example_interfaces.AddTwoIntsResponse{Sum: request.A + request.B}, nil
		})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client, err := node.CreateServiceClient("/generated_add").Build(&example_interfaces.AddTwoInts{})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := client.WaitForServiceWithContext(ctx); err != nil {
		t.Fatal(err)
	}
	var response example_interfaces.AddTwoIntsResponse
	err = hiroz.CallTypedWithContext(ctx, client, &example_interfaces.AddTwoIntsRequest{A: 6, B: 7}, &response)
	if err != nil || response.Sum != 13 {
		t.Fatalf("response=%#v, err=%v", response, err)
	}
}

func TestGeneratedTypedActionFeedbackAndResult(t *testing.T) {
	node := generatedNode(t)
	server, err := hiroz.BuildTypedActionServer(node.CreateActionServer("/generated_fibonacci"), &example_interfaces.Fibonacci{},
		func(goal *example_interfaces.FibonacciGoal) bool { return goal.Order == 5 },
		func(handle *hiroz.ServerGoalHandle, goal *example_interfaces.FibonacciGoal) (*example_interfaces.FibonacciResult, error) {
			sequence := []int32{0, 1, 1, 2, 3}
			if err := handle.PublishFeedback(&example_interfaces.FibonacciFeedback{Sequence: sequence}); err != nil {
				return nil, err
			}
			return &example_interfaces.FibonacciResult{Sequence: sequence}, nil
		})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client, err := node.CreateActionClient("/generated_fibonacci").Build(&example_interfaces.Fibonacci{})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if err := client.WaitForServer(ctx); err != nil {
		t.Fatal(err)
	}
	goal, err := client.SendGoalWithContext(ctx, &example_interfaces.FibonacciGoal{Order: 5})
	if err != nil {
		t.Fatal(err)
	}
	defer goal.Close()
	var feedback example_interfaces.FibonacciFeedback
	if err := hiroz.ReceiveTypedFeedback(ctx, goal, &feedback); err != nil {
		t.Fatal(err)
	}
	var result example_interfaces.FibonacciResult
	if err := hiroz.GetTypedResultWithContext(ctx, goal, &result); err != nil {
		t.Fatal(err)
	}
	expected := []int32{0, 1, 1, 2, 3}
	if !reflect.DeepEqual(feedback.Sequence, expected) || !reflect.DeepEqual(result.Sequence, expected) || goal.Status() != hiroz.GoalStatusSucceeded {
		t.Fatalf("feedback=%v result=%v status=%v", feedback.Sequence, result.Sequence, goal.Status())
	}
}
