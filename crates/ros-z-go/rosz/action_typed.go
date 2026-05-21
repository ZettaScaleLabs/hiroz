package rosz

import "fmt"

// BuildTypedActionServer creates an action server with typed goal/feedback/result handling.
// The goal callback receives the deserialized goal and returns bool (accept/reject).
// The execute callback receives a ServerGoalHandle and the deserialized goal, and must
// return a serialized result.
//
// Example:
//
//	server, err := rosz.BuildTypedActionServer(
//	    node.CreateActionServer("fibonacci"),
//	    &example_interfaces.Fibonacci{},
//	    func(goal *example_interfaces.FibonacciGoal) bool { return true },
//	    func(h *rosz.ServerGoalHandle, goal *example_interfaces.FibonacciGoal) (*example_interfaces.FibonacciResult, error) {
//	        seq := []int32{0, 1}
//	        for i := 2; i < int(goal.Order); i++ {
//	            if h.IsCancelRequested() { return &example_interfaces.FibonacciResult{Sequence: seq}, nil }
//	            seq = append(seq, seq[i-1]+seq[i-2])
//	        }
//	        return &example_interfaces.FibonacciResult{Sequence: seq}, nil
//	    },
//	)
func BuildTypedActionServer[Goal, Result Message](
	builder *ActionServerBuilder,
	action Action,
	goalCallback func(Goal) bool,
	executeCallback func(h *ServerGoalHandle, goal Goal) (Result, error),
) (*ActionServer, error) {
	rawGoalCallback := func(goalBytes []byte) bool {
		var goal Goal
		if err := goal.DeserializeCDR(goalBytes); err != nil {
			return false
		}
		return goalCallback(goal)
	}

	rawExecuteCallback := func(h *ServerGoalHandle, goalBytes []byte) ([]byte, error) {
		var goal Goal
		if err := goal.DeserializeCDR(goalBytes); err != nil {
			return nil, fmt.Errorf("failed to deserialize goal: %w", err)
		}
		result, err := executeCallback(h, goal)
		if err != nil {
			return nil, err
		}
		return result.SerializeCDR()
	}

	return builder.Build(action, rawGoalCallback, rawExecuteCallback)
}

// GetTypedResult waits for the goal result and deserializes it into the provided result template.
//
// Example:
//
//	result := &example_interfaces.FibonacciResult{}
//	err := rosz.GetTypedResult(handle, result)
func GetTypedResult[Result Message](handle *GoalHandle, result Result) error {
	resultBytes, err := handle.GetResult()
	if err != nil {
		return err
	}
	if err := result.DeserializeCDR(resultBytes); err != nil {
		return newRoszError(ErrorCodeDeserializationFailed, fmt.Sprintf("failed to deserialize result: %v", err))
	}
	return nil
}
