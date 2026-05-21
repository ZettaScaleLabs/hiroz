package rosz

import (
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
//	err := rosz.CallTyped(client, &example_interfaces.AddTwoIntsRequest{A: 1, B: 2}, resp)
//	fmt.Println(resp.Sum)
func CallTyped[Resp Message](client *ServiceClient, request Message, response Resp) error {
	respBytes, err := client.call(request)
	if err != nil {
		return err
	}
	if err := response.DeserializeCDR(respBytes); err != nil {
		return newRoszError(ErrorCodeDeserializationFailed, fmt.Sprintf("failed to deserialize response: %v", err))
	}
	return nil
}

// CallTypedWithTimeout makes a typed service call with a custom timeout.
//
// Example:
//
//	resp := &example_interfaces.AddTwoIntsResponse{}
//	err := rosz.CallTypedWithTimeout(client, req, resp, 10*time.Second)
func CallTypedWithTimeout[Resp Message](client *ServiceClient, request Message, response Resp, timeout time.Duration) error {
	respBytes, err := client.callWithTimeout(request, timeout)
	if err != nil {
		return err
	}
	if err := response.DeserializeCDR(respBytes); err != nil {
		return newRoszError(ErrorCodeDeserializationFailed, fmt.Sprintf("failed to deserialize response: %v", err))
	}
	return nil
}

// BuildTypedServiceServer creates a service server with typed request/response handling.
// The callback receives an already-deserialized request and returns a response message.
//
// Example:
//
//	server, err := rosz.BuildTypedServiceServer(
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
	rawCallback := func(reqBytes []byte) ([]byte, error) {
		var req Req
		if err := req.DeserializeCDR(reqBytes); err != nil {
			return nil, fmt.Errorf("failed to deserialize request: %w", err)
		}
		resp, err := callback(req)
		if err != nil {
			return nil, err
		}
		return resp.SerializeCDR()
	}
	return builder.Build(svc, rawCallback)
}
