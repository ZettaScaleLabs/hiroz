package hiroz

import (
	"context"
	"errors"
)

// ReceiveTypedFeedback receives and decodes the next feedback message into an
// allocated destination. A separate destination per concurrent receiver is required.
func ReceiveTypedFeedback(ctx context.Context, handle *GoalHandle, feedback Message) error {
	if ctx == nil {
		return errors.New("nil context")
	}
	if err := actionContextError(ctx); err != nil {
		return err
	}
	select {
	case <-ctx.Done():
		return ctx.Err()
	case bytes, ok := <-handle.Feedback():
		if !ok {
			return errors.New("goal handle is closed")
		}
		if err := actionContextError(ctx); err != nil {
			return err
		}
		return feedback.DeserializeCDR(bytes)
	}
}

// GetTypedResultWithContext decodes a result while allowing cancellation of the
// local wait. The terminal outcome is available through handle.Status().
func GetTypedResultWithContext[Result Message](ctx context.Context, handle *GoalHandle, result Result) error {
	bytes, err := handle.GetResultWithContext(ctx)
	if err != nil {
		return err
	}
	return result.DeserializeCDR(bytes)
}
