package hiroz

import (
	"context"
	"fmt"
	"time"
)

// CallTyped makes a typed service call with automatic serialization and deserialization.
// The request is serialized, sent with the default 5-second timeout, and the response
// is deserialized into the provided response template.
//
// Example:
//
//	resp := &example_interfaces.AddTwoIntsResponse{}
//	err := hiroz.CallTyped(client, &example_interfaces.AddTwoIntsRequest{A: 1, B: 2}, resp)
//	fmt.Println(resp.Sum)
func CallTyped[Resp Message](client *ServiceClient, request Message, response Resp) error {
	respBytes, err := client.call(request)
	if err != nil {
		return err
	}
	if err := response.DeserializeCDR(respBytes); err != nil {
		return newHirozError(ErrorCodeDeserializationFailed, fmt.Sprintf("failed to deserialize response: %v", err))
	}
	return nil
}

// CallTypedWithTimeout makes a typed service call with a custom timeout.
//
// Example:
//
//	resp := &example_interfaces.AddTwoIntsResponse{}
//	err := hiroz.CallTypedWithTimeout(client, req, resp, 10*time.Second)
func CallTypedWithTimeout[Resp Message](client *ServiceClient, request Message, response Resp, timeout time.Duration) error {
	respBytes, err := client.callWithTimeout(request, timeout)
	if err != nil {
		return err
	}
	if err := response.DeserializeCDR(respBytes); err != nil {
		return newHirozError(ErrorCodeDeserializationFailed, fmt.Sprintf("failed to deserialize response: %v", err))
	}
	return nil
}

// CallTypedContext calls the service, allowing ctx to cancel the wait for its response.
// Serialization and decoding run synchronously and are not interrupted. Cancellation does not
// stop the underlying C call; ServiceClient.Close waits for that call.
//
// A context deadline sets the timeout of the call, up to 10 seconds, the limit of every query
// of the service client. Without a deadline the call uses DefaultServiceTimeout. When ctx ends
// first, the error wraps ctx.Err() and does not match ErrTimeout. A call timeout that ends
// after the context deadline has passed also returns context.DeadlineExceeded. ErrTimeout
// means the server did not answer within the call timeout while the deadline was still ahead.
//
// A call whose ctx ended during request.SerializeCDR returns the context error when the
// serializer returns, and does not reach the C layer. ctx is checked once after the C result
// arrives, before response.DeserializeCDR runs. If ctx ends during decoding, the call returns
// nil and response holds the decoded value.
func CallTypedContext[Resp Message](ctx context.Context, client *ServiceClient, request Message, response Resp) error {
	// Check before the user's serializer runs, so a cancelled call does not wait for it.
	if err := ctx.Err(); err != nil {
		return err
	}
	reqBytes, err := request.SerializeCDR()
	// ctx may have ended while the serializer ran. The context error wins over a serializer
	// error.
	if cerr := ctx.Err(); cerr != nil {
		return cerr
	}
	if err != nil {
		return fmt.Errorf("failed to serialize request: %w", err)
	}
	respBytes, err := client.callRawContext(ctx, reqBytes)
	if err != nil {
		return err
	}
	if err := response.DeserializeCDR(respBytes); err != nil {
		return newHirozError(ErrorCodeDeserializationFailed, fmt.Sprintf("failed to deserialize response: %v", err))
	}
	return nil
}

// BuildTypedServiceServer creates a service server with typed request/response handling.
// The callback receives an already-deserialized request and returns a response message.
//
// Example:
//
//	server, err := hiroz.BuildTypedServiceServer(
//	    node.CreateServiceServer("add_two_ints"),
//	    &example_interfaces.AddTwoInts{},
//	    func(req *example_interfaces.AddTwoIntsRequest) (*example_interfaces.AddTwoIntsResponse, error) {
//	        return &example_interfaces.AddTwoIntsResponse{Sum: req.A + req.B}, nil
//	    },
//	)
func BuildTypedServiceServer[Req, Resp Message](
	builder *ServiceServerBuilder,
	svc Service,
	callback func(Req) (Resp, error),
) (*ServiceServer, error) {
	rawCallback := typedServiceCallback(callback)
	return builder.Build(svc, rawCallback)
}

// typedServiceCallback adapts a typed service callback to the raw byte form.
func typedServiceCallback[Req, Resp Message](callback func(Req) (Resp, error)) func([]byte) ([]byte, error) {
	return func(reqBytes []byte) ([]byte, error) {
		req := newMessage[Req]()
		if err := req.DeserializeCDR(reqBytes); err != nil {
			return nil, fmt.Errorf("failed to deserialize request: %w", err)
		}
		resp, err := callback(req)
		if err != nil {
			return nil, err
		}
		return resp.SerializeCDR()
	}
}
