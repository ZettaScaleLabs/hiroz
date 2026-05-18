package hiroz

import (
	"fmt"
	"log/slog"
	"os"
	"runtime/debug"
)

// logger is the package-level structured logger.
// Set HIROZ_LOG=DEBUG|INFO|WARN|ERROR to control verbosity at runtime.
// Default is WARN so production binaries are silent.
var logger = func() *slog.Logger {
	level := slog.LevelWarn
	if v := os.Getenv("HIROZ_LOG"); v != "" {
		_ = level.UnmarshalText([]byte(v))
	}
	return slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: level}))
}()

// safeCall runs fn and recovers any panic, returning it as an error.
// Use this in every //export function to prevent Go panics from crossing
// the CGo boundary (which is undefined behaviour).
func safeCall(fn func() error) (err error) {
	defer func() {
		if r := recover(); r != nil {
			logger.Error("panic in CGo callback",
				"recover", fmt.Sprintf("%v", r),
				"stack", string(debug.Stack()))
			err = fmt.Errorf("panic in callback: %v", r)
		}
	}()
	return fn()
}
