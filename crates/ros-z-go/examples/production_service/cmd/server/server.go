package main

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/examples/production_service/messages"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
	"golang.org/x/time/rate"
)

// CacheStore represents a thread-safe in-memory cache (simulating a database)
type CacheStore struct {
	mu    sync.RWMutex
	store map[string]int64
	stats CacheStats
}

type CacheStats struct {
	totalReads  atomic.Uint64
	totalWrites atomic.Uint64
	cacheHits   atomic.Uint64
	cacheMisses atomic.Uint64
	errors      atomic.Uint64
}

func NewCacheStore() *CacheStore {
	return &CacheStore{
		store: make(map[string]int64),
	}
}

func (cs *CacheStore) Get(key string) (int64, bool) {
	cs.mu.RLock()
	defer cs.mu.RUnlock()

	cs.stats.totalReads.Add(1)
	val, ok := cs.store[key]
	if ok {
		cs.stats.cacheHits.Add(1)
	} else {
		cs.stats.cacheMisses.Add(1)
	}
	return val, ok
}

func (cs *CacheStore) Set(key string, value int64) {
	cs.mu.Lock()
	defer cs.mu.Unlock()

	cs.stats.totalWrites.Add(1)
	cs.store[key] = value
}

func (cs *CacheStore) Size() int {
	cs.mu.RLock()
	defer cs.mu.RUnlock()
	return len(cs.store)
}

func (cs *CacheStore) Stats() map[string]uint64 {
	return map[string]uint64{
		"total_reads":  cs.stats.totalReads.Load(),
		"total_writes": cs.stats.totalWrites.Load(),
		"cache_hits":   cs.stats.cacheHits.Load(),
		"cache_misses": cs.stats.cacheMisses.Load(),
		"errors":       cs.stats.errors.Load(),
		"cache_size":   uint64(cs.Size()),
	}
}

// ProductionServiceServer implements a production-ready ROS 2 service
type ProductionServiceServer struct {
	logger      *slog.Logger
	cache       *CacheStore
	rateLimiter *rate.Limiter
	ctx         context.Context
	cancel      context.CancelFunc
	rosCtx      *rosz.Context
	node        *rosz.Node
	server      *rosz.ServiceServer
	wg          sync.WaitGroup
}

func NewProductionServiceServer() (*ProductionServiceServer, error) {
	// Structured logging with JSON output
	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	}))

	// Rate limiter: 100 requests/sec with burst of 10
	limiter := rate.NewLimiter(100, 10)

	ctx, cancel := context.WithCancel(context.Background())

	return &ProductionServiceServer{
		logger:      logger,
		cache:       NewCacheStore(),
		rateLimiter: limiter,
		ctx:         ctx,
		cancel:      cancel,
	}, nil
}

func (s *ProductionServiceServer) Start() error {
	// Create ROS-Z context and node
	rosCtx, err := rosz.NewContext().WithDomainID(0).Build()
	if err != nil {
		return fmt.Errorf("failed to create context: %w", err)
	}
	s.rosCtx = rosCtx // store immediately so Shutdown() can close it on any failure

	node, err := rosCtx.CreateNode("production_cache_server").Build()
	if err != nil {
		s.rosCtx.Close()
		s.rosCtx = nil
		return fmt.Errorf("failed to create node: %w", err)
	}
	s.node = node

	// Create service with error recovery
	svc := &messages.AddTwoInts{}
	server, err := node.CreateServiceServer("cache_service").
		Build(svc, s.handleRequest)
	if err != nil {
		return fmt.Errorf("failed to create service: %w", err)
	}
	s.server = server

	s.logger.Info("Service started",
		"service", "cache_service",
		"rate_limit", "100 req/s",
		"burst", 10,
	)

	// Start metrics reporter
	s.wg.Add(1)
	go s.reportMetrics()

	// Start health monitor
	s.wg.Add(1)
	go s.healthMonitor()

	return nil
}

// handleRequest implements the service handler with production patterns
func (s *ProductionServiceServer) handleRequest(reqData []byte) ([]byte, error) {
	startTime := time.Now()

	// Panic recovery
	defer func() {
		if r := recover(); r != nil {
			s.cache.stats.errors.Add(1)
			s.logger.Error("Panic recovered in service handler",
				"panic", r,
				"duration_ms", time.Since(startTime).Milliseconds(),
			)
		}
	}()

	// Rate limiting
	if !s.rateLimiter.Allow() {
		s.cache.stats.errors.Add(1)
		s.logger.Warn("Request rate limited",
			"duration_ms", time.Since(startTime).Milliseconds(),
		)
		return nil, fmt.Errorf("rate limit exceeded")
	}

	// Deserialize request
	var req messages.AddTwoIntsRequest
	if err := req.DeserializeCDR(reqData); err != nil {
		s.cache.stats.errors.Add(1)
		s.logger.Error("Failed to deserialize request",
			"error", err,
			"duration_ms", time.Since(startTime).Milliseconds(),
		)
		return nil, fmt.Errorf("failed to deserialize request: %w", err)
	}

	// Business logic: Use 'a' as cache key, 'b' as operation code
	// b=0: get, b=1: set/increment
	cacheKey := fmt.Sprintf("key_%d", req.A)

	var result int64
	if req.B == 0 {
		// GET operation
		val, ok := s.cache.Get(cacheKey)
		if ok {
			result = val
			s.logger.Debug("Cache hit",
				"key", cacheKey,
				"value", result,
			)
		} else {
			result = 0
			s.logger.Debug("Cache miss",
				"key", cacheKey,
			)
		}
	} else {
		// SET/INCREMENT operation
		currentVal, _ := s.cache.Get(cacheKey)
		result = currentVal + req.B
		s.cache.Set(cacheKey, result)
		s.logger.Debug("Cache updated",
			"key", cacheKey,
			"new_value", result,
		)
	}

	// Serialize response
	resp := &messages.AddTwoIntsResponse{Sum: result}
	respData, err := resp.SerializeCDR()
	if err != nil {
		s.cache.stats.errors.Add(1)
		s.logger.Error("Failed to serialize response",
			"error", err,
			"duration_ms", time.Since(startTime).Milliseconds(),
		)
		return nil, fmt.Errorf("failed to serialize response: %w", err)
	}

	s.logger.Info("Request processed",
		"key", cacheKey,
		"operation", map[int64]string{0: "GET", 1: "SET"}[min(req.B, 1)],
		"result", result,
		"duration_ms", time.Since(startTime).Milliseconds(),
	)

	return respData, nil
}

// reportMetrics periodically logs cache statistics
func (s *ProductionServiceServer) reportMetrics() {
	defer s.wg.Done()
	ticker := time.NewTicker(10 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-s.ctx.Done():
			s.logger.Info("Metrics reporter shutting down")
			return
		case <-ticker.C:
			stats := s.cache.Stats()
			s.logger.Info("Cache metrics",
				"total_reads", stats["total_reads"],
				"total_writes", stats["total_writes"],
				"cache_hits", stats["cache_hits"],
				"cache_misses", stats["cache_misses"],
				"errors", stats["errors"],
				"cache_size", stats["cache_size"],
				"hit_rate", s.calculateHitRate(stats),
			)
		}
	}
}

func (s *ProductionServiceServer) calculateHitRate(stats map[string]uint64) string {
	totalReads := stats["total_reads"]
	if totalReads == 0 {
		return "N/A"
	}
	hitRate := float64(stats["cache_hits"]) / float64(totalReads) * 100
	return fmt.Sprintf("%.2f%%", hitRate)
}

// healthMonitor checks service health and logs status
func (s *ProductionServiceServer) healthMonitor() {
	defer s.wg.Done()
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-s.ctx.Done():
			s.logger.Info("Health monitor shutting down")
			return
		case <-ticker.C:
			stats := s.cache.Stats()
			errorRate := float64(stats["errors"]) / float64(max(stats["total_reads"]+stats["total_writes"], 1)) * 100

			health := "healthy"
			if errorRate > 5.0 {
				health = "degraded"
			}

			s.logger.Info("Health check",
				"status", health,
				"error_rate", fmt.Sprintf("%.2f%%", errorRate),
				"uptime", time.Since(time.Now().Add(-30*time.Second)),
			)
		}
	}
}

// Shutdown gracefully stops the service
func (s *ProductionServiceServer) Shutdown() {
	s.logger.Info("Initiating graceful shutdown...")

	// Signal goroutines to stop
	s.cancel()

	// Close ROS-Z resources
	if s.server != nil {
		s.server.Close()
		s.logger.Info("Service server closed")
	}
	if s.node != nil {
		s.node.Close()
		s.logger.Info("Node closed")
	}
	if s.rosCtx != nil {
		s.rosCtx.Close()
		s.logger.Info("Context closed")
	}

	// Wait for goroutines with timeout
	done := make(chan struct{})
	go func() {
		s.wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		s.logger.Info("All goroutines stopped")
	case <-time.After(5 * time.Second):
		s.logger.Warn("Shutdown timeout, some goroutines may still be running")
	}

	// Final stats
	stats := s.cache.Stats()
	s.logger.Info("Final statistics",
		"total_requests", stats["total_reads"]+stats["total_writes"],
		"cache_size", stats["cache_size"],
		"total_errors", stats["errors"],
	)
}

func main() {
	server, err := NewProductionServiceServer()
	if err != nil {
		slog.Error("Failed to create server", "error", err)
		os.Exit(1)
	}

	if err := server.Start(); err != nil {
		slog.Error("Failed to start server", "error", err)
		os.Exit(1)
	}

	// Setup signal handling for graceful shutdown
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	slog.Info("Server running. Press Ctrl+C to shutdown gracefully.")

	// Wait for shutdown signal
	sig := <-sigChan
	slog.Info("Received shutdown signal", "signal", sig)

	server.Shutdown()
	slog.Info("Server stopped")
}

