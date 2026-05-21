package main

import (
	"context"
	"fmt"
	"log/slog"
	"math/rand"
	"os"
	"os/signal"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/examples/production_service/messages"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

// ClientMetrics tracks client-side statistics
type ClientMetrics struct {
	totalRequests   atomic.Uint64
	successfulCalls atomic.Uint64
	failedCalls     atomic.Uint64
	retriedCalls    atomic.Uint64
	totalLatencyMs  atomic.Uint64
}

func (m *ClientMetrics) RecordSuccess(latency time.Duration) {
	m.totalRequests.Add(1)
	m.successfulCalls.Add(1)
	m.totalLatencyMs.Add(uint64(latency.Milliseconds()))
}

func (m *ClientMetrics) RecordFailure() {
	m.totalRequests.Add(1)
	m.failedCalls.Add(1)
}

func (m *ClientMetrics) RecordRetry() {
	m.retriedCalls.Add(1)
}

func (m *ClientMetrics) AverageLatency() time.Duration {
	total := m.totalRequests.Load()
	if total == 0 {
		return 0
	}
	avgMs := m.totalLatencyMs.Load() / total
	return time.Duration(avgMs) * time.Millisecond
}

// ProductionClient implements production-ready service client patterns
type ProductionClient struct {
	logger  *slog.Logger
	client  *rosz.ServiceClient
	metrics *ClientMetrics
	ctx     context.Context
	cancel  context.CancelFunc
}

// RetryConfig defines retry behavior
type RetryConfig struct {
	MaxRetries     int
	InitialBackoff time.Duration
	MaxBackoff     time.Duration
	Multiplier     float64
}

var DefaultRetryConfig = RetryConfig{
	MaxRetries:     3,
	InitialBackoff: 100 * time.Millisecond,
	MaxBackoff:     2 * time.Second,
	Multiplier:     2.0,
}

func NewProductionClient(ctx context.Context) (*ProductionClient, error) {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	}))

	childCtx, cancel := context.WithCancel(ctx)

	// Create ROS-Z context and node
	rosCtx, err := rosz.NewContext().WithDomainID(0).Build()
	if err != nil {
		cancel()
		return nil, fmt.Errorf("failed to create context: %w", err)
	}

	success := false
	defer func() {
		if !success {
			rosCtx.Close()
		}
	}()

	node, err := rosCtx.CreateNode("production_cache_client").Build()
	if err != nil {
		cancel()
		return nil, fmt.Errorf("failed to create node: %w", err)
	}

	// Create service client
	svc := &messages.AddTwoInts{}
	client, err := node.CreateServiceClient("cache_service").Build(svc)
	if err != nil {
		cancel()
		return nil, fmt.Errorf("failed to create client: %w", err)
	}

	logger.Info("Client initialized",
		"service", "cache_service",
		"retry_config", DefaultRetryConfig,
	)

	success = true
	return &ProductionClient{
		logger:  logger,
		client:  client,
		metrics: &ClientMetrics{},
		ctx:     childCtx,
		cancel:  cancel,
	}, nil
}

// CallWithRetry implements exponential backoff retry logic
func (c *ProductionClient) CallWithRetry(req *messages.AddTwoIntsRequest, config RetryConfig) (*messages.AddTwoIntsResponse, error) {
	var lastErr error
	backoff := config.InitialBackoff

	for attempt := 0; attempt <= config.MaxRetries; attempt++ {
		// Check context cancellation
		select {
		case <-c.ctx.Done():
			return nil, c.ctx.Err()
		default:
		}

		startTime := time.Now()

		// Make the call
		var resp messages.AddTwoIntsResponse
		err := rosz.CallTyped(c.client, req, &resp)
		latency := time.Since(startTime)

		if err == nil {
			// Success!
			c.metrics.RecordSuccess(latency)
			c.logger.Info("Request successful",
				"attempt", attempt+1,
				"latency_ms", latency.Milliseconds(),
				"result", resp.Sum,
			)
			return &resp, nil
		}

		// Handle error
		lastErr = err
		c.logger.Warn("Request failed",
			"attempt", attempt+1,
			"max_attempts", config.MaxRetries+1,
			"error", err,
			"latency_ms", latency.Milliseconds(),
		)

		// Check if we should retry
		if attempt < config.MaxRetries {
			c.metrics.RecordRetry()

			// Exponential backoff with jitter
			jitter := time.Duration(rand.Int63n(int64(backoff / 4)))
			sleepTime := backoff + jitter

			c.logger.Info("Retrying after backoff",
				"backoff_ms", sleepTime.Milliseconds(),
				"next_attempt", attempt+2,
			)

			select {
			case <-c.ctx.Done():
				return nil, c.ctx.Err()
			case <-time.After(sleepTime):
			}

			// Increase backoff
			backoff = time.Duration(float64(backoff) * config.Multiplier)
			if backoff > config.MaxBackoff {
				backoff = config.MaxBackoff
			}
		}
	}

	c.metrics.RecordFailure()
	c.logger.Error("All retry attempts exhausted",
		"max_attempts", config.MaxRetries+1,
		"last_error", lastErr,
	)
	return nil, fmt.Errorf("all retry attempts failed: %w", lastErr)
}

// RunWorkload simulates a production workload
func (c *ProductionClient) RunWorkload() {
	ticker := time.NewTicker(500 * time.Millisecond)
	defer ticker.Stop()

	reportTicker := time.NewTicker(5 * time.Second)
	defer reportTicker.Stop()

	keyCounter := int64(0)

	for {
		select {
		case <-c.ctx.Done():
			c.logger.Info("Workload stopped")
			return

		case <-ticker.C:
			// Simulate cache operations: 70% reads, 30% writes
			operation := int64(0) // GET
			if rand.Float64() < 0.3 {
				operation = 1 // SET/INCREMENT
			}

			req := &messages.AddTwoIntsRequest{
				A: keyCounter % 10, // Cycle through 10 cache keys
				B: operation,
			}

			go func(r *messages.AddTwoIntsRequest) {
				_, _ = c.CallWithRetry(r, DefaultRetryConfig)
			}(req)

			keyCounter++

		case <-reportTicker.C:
			c.reportMetrics()
		}
	}
}

func (c *ProductionClient) reportMetrics() {
	total := c.metrics.totalRequests.Load()
	success := c.metrics.successfulCalls.Load()
	failed := c.metrics.failedCalls.Load()
	retries := c.metrics.retriedCalls.Load()
	avgLatency := c.metrics.AverageLatency()

	successRate := 0.0
	if total > 0 {
		successRate = float64(success) / float64(total) * 100
	}

	c.logger.Info("Client metrics",
		"total_requests", total,
		"successful", success,
		"failed", failed,
		"retries", retries,
		"success_rate", fmt.Sprintf("%.2f%%", successRate),
		"avg_latency_ms", avgLatency.Milliseconds(),
	)
}

func (c *ProductionClient) Shutdown() {
	c.logger.Info("Client shutting down")
	c.cancel()

	if c.client != nil {
		c.client.Close()
	}

	// Final metrics report
	c.reportMetrics()
}

func main() {
	// Setup context with cancellation
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	client, err := NewProductionClient(ctx)
	if err != nil {
		slog.Error("Failed to create client", "error", err)
		os.Exit(1)
	}

	// Setup signal handling
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	// Run workload in background
	go client.RunWorkload()

	slog.Info("Client running. Press Ctrl+C to shutdown gracefully.")

	// Wait for shutdown signal
	sig := <-sigChan
	slog.Info("Received shutdown signal", "signal", sig)

	client.Shutdown()
	slog.Info("Client stopped")
}
