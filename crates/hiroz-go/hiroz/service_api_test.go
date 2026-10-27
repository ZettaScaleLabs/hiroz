package hiroz

import (
	"bytes"
	"context"
	"errors"
	"testing"
	"time"
)

type liveService struct{}

func (*liveService) TypeName() string { return "test::srv::dds_::Raw_" }
func (*liveService) TypeHash() string {
	return "RIHS01_0000000000000000000000000000000000000000000000000000000000000000"
}
func (*liveService) GetRequest() Message  { return &MockMessage{} }
func (*liveService) GetResponse() Message { return &MockMessage{} }

func serviceTestNode(t *testing.T) *Node {
	t.Helper()
	ctx, err := NewContext().DisableMulticastScouting().WithJSON(`{"listen/endpoints":[],"scouting/gossip/enabled":false}`).Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ctx.Close() })
	node, err := ctx.CreateNode("go_service_api").Build()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { node.Close() })
	return node
}

func serviceTestPair(t *testing.T, name string) (*ServiceClient, *ServiceServer) {
	t.Helper()
	node := serviceTestNode(t)
	server, err := node.CreateServiceServer(name).WithQoS(QosServicesDefault()).BuildQueued(&liveService{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { server.Close() })
	client, err := node.CreateServiceClient(name).WithQoS(QosServicesDefault()).Build(&liveService{})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { client.Close() })
	if err := client.WaitForService(2 * time.Second); err != nil {
		t.Fatal(err)
	}
	return client, server
}

type serviceCallResult struct {
	data []byte
	err  error
}

func asyncServiceCall(client *ServiceClient, ctx context.Context, data []byte) <-chan serviceCallResult {
	ch := make(chan serviceCallResult, 1)
	go func() {
		data, err := client.CallRawWithContext(ctx, data)
		ch <- serviceCallResult{data, err}
	}()
	return ch
}

func takeServiceRequest(t *testing.T, server *ServiceServer) *ServiceRequest {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	request, err := server.TakeRequest(ctx)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { request.Close() })
	return request
}

func TestServiceDeferredReplyRetainsRequestAfterServerClose(t *testing.T) {
	client, server := serviceTestPair(t, "/deferred_reply")
	if request, err := server.TryTakeRequest(); err != nil || request != nil {
		t.Fatalf("empty queue: request=%v, err=%v", request, err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	result := asyncServiceCall(client, ctx, []byte{0, 1, 0, 0, 7})
	request := takeServiceRequest(t, server)
	if !bytes.Equal(request.Data, []byte{0, 1, 0, 0, 7}) {
		t.Fatalf("request: %v", request.Data)
	}
	if request.ID.WriterGID == [16]byte{} {
		t.Fatal("request has no writer identity")
	}
	if err := server.Close(); err != nil {
		t.Fatal(err)
	}
	response := []byte{0, 1, 0, 0, 8}
	if err := request.ReplyRaw(response); err != nil {
		t.Fatal(err)
	}
	if err := request.ReplyRaw(response); err == nil {
		t.Fatal("allowed two replies for one request")
	}
	got := <-result
	if got.err != nil || !bytes.Equal(got.data, response) {
		t.Fatalf("response=%v, err=%v", got.data, got.err)
	}
}

func TestServiceContextCancellationAndClientClose(t *testing.T) {
	for _, mode := range []string{"cancel", "close"} {
		t.Run(mode, func(t *testing.T) {
			client, server := serviceTestPair(t, "/cancel_"+mode)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			result := asyncServiceCall(client, ctx, []byte{1})
			request := takeServiceRequest(t, server)
			if mode == "cancel" {
				cancel()
			} else if err := client.Close(); err != nil {
				t.Fatal(err)
			}
			select {
			case got := <-result:
				if !errors.Is(got.err, context.Canceled) {
					t.Fatalf("error=%v, want cancellation", got.err)
				}
			case <-time.After(time.Second):
				t.Fatal("canceled call did not return promptly")
			}
			_ = request.ReplyRaw([]byte{9}) // Late reply must not reanimate a canceled call.
		})
	}
}

func TestServiceConcurrentCallsDoNotSwapResponses(t *testing.T) {
	client, server := serviceTestPair(t, "/concurrent_calls")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	a := asyncServiceCall(client, ctx, []byte{1})
	b := asyncServiceCall(client, ctx, []byte{2})
	first := takeServiceRequest(t, server)
	second := takeServiceRequest(t, server)
	if first.ID == second.ID {
		t.Fatal("requests reused the same identity")
	}
	if err := second.ReplyRaw(second.Data); err != nil {
		t.Fatal(err)
	}
	if err := first.ReplyRaw(first.Data); err != nil {
		t.Fatal(err)
	}
	for i, ch := range []<-chan serviceCallResult{a, b} {
		got := <-ch
		if got.err != nil || !bytes.Equal(got.data, []byte{byte(i + 1)}) {
			t.Fatalf("call %d: response=%v, err=%v", i, got.data, got.err)
		}
	}
}

func TestServiceCallbackCanCloseItsServer(t *testing.T) {
	node := serviceTestNode(t)
	var server *ServiceServer
	var err error
	server, err = node.CreateServiceServer("/callback_close").Build(&liveService{}, func(data []byte) ([]byte, error) {
		return data, server.Close()
	})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client, err := node.CreateServiceClient("/callback_close").Build(&liveService{})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	if err := client.WaitForService(time.Second); err != nil {
		t.Fatal(err)
	}
	got, err := client.CallRawWithTimeout([]byte{3}, 2*time.Second)
	if err != nil || !bytes.Equal(got, []byte{3}) {
		t.Fatalf("response=%v, err=%v", got, err)
	}
}

func TestServiceTransportHonorsTimeoutBeyondTenSeconds(t *testing.T) {
	if testing.Short() {
		t.Skip("exercises the former 10-second transport deadline")
	}
	client, server := serviceTestPair(t, "/long_timeout")
	result := make(chan serviceCallResult, 1)
	go func() {
		data, err := client.CallRawWithTimeout([]byte{1}, 15*time.Second)
		result <- serviceCallResult{data, err}
	}()
	request := takeServiceRequest(t, server)
	timer := time.NewTimer(11 * time.Second)
	defer timer.Stop()
	select {
	case got := <-result:
		t.Fatalf("call ended before its declared timeout: %v", got.err)
	case <-timer.C:
	}
	if err := request.ReplyRaw([]byte{2}); err != nil {
		t.Fatal(err)
	}
	got := <-result
	if got.err != nil || !bytes.Equal(got.data, []byte{2}) {
		t.Fatalf("response=%v, err=%v", got.data, got.err)
	}
}

func TestServiceDurationAndContextValidation(t *testing.T) {
	client := &ServiceClient{}
	for _, timeout := range []time.Duration{-time.Second, 0} {
		if _, err := client.CallRawWithTimeout([]byte{1}, timeout); err == nil {
			t.Fatalf("accepted timeout %v", timeout)
		}
	}
	if err := client.WaitForService(-time.Second); err == nil {
		t.Fatal("accepted negative readiness timeout")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := client.CallRawWithContext(ctx, []byte{1}); !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled context: %v", err)
	}
}
