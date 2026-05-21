package rosz

/*
#cgo LDFLAGS: -L${SRCDIR}/../../../target/release -lros_z -lm
#cgo linux LDFLAGS: -ldl -lpthread
#cgo darwin LDFLAGS: -framework Security -framework CoreFoundation -framework SystemConfiguration -framework IOKit
#include <stdlib.h>
#include "ros_z_ffi.h"
*/
import "C"
import (
	"fmt"
	"runtime"
	"sync"
	"unsafe"
)

// ZenohMode represents the Zenoh session mode
type ZenohMode string

const (
	// ModePeer connects as a Zenoh peer (default)
	ModePeer ZenohMode = "peer"
	// ModeClient connects as a Zenoh client
	ModeClient ZenohMode = "client"
	// ModeRouter connects as a Zenoh router
	ModeRouter ZenohMode = "router"
)

// Context represents a ros-z context
type Context struct {
	handle    *C.ros_z_context_t
	closeOnce sync.Once
}

// ContextBuilder builds a Context
type ContextBuilder struct {
	domainID             uint32
	configFile           string
	connectEndpoints     []string
	mode                 ZenohMode
	disableMulticast     bool
	connectToLocalZenohd bool
	jsonConfig           string
	remapRules           []string
	enableLogging        bool
	hasAdvancedConfig    bool
}

// NewContext creates a new context builder
func NewContext() *ContextBuilder {
	return &ContextBuilder{
		domainID: 0,
	}
}

// WithDomainID sets the ROS domain ID
func (b *ContextBuilder) WithDomainID(id uint32) *ContextBuilder {
	b.domainID = id
	return b
}

// WithConfigFile loads Zenoh configuration from a JSON5 file
func (b *ContextBuilder) WithConfigFile(path string) *ContextBuilder {
	b.configFile = path
	b.hasAdvancedConfig = true
	return b
}

// WithConnectEndpoints sets Zenoh connect endpoints
func (b *ContextBuilder) WithConnectEndpoints(endpoints ...string) *ContextBuilder {
	b.connectEndpoints = endpoints
	b.hasAdvancedConfig = true
	return b
}

// DisableMulticastScouting disables multicast scouting (useful in Docker/cloud)
func (b *ContextBuilder) DisableMulticastScouting() *ContextBuilder {
	b.disableMulticast = true
	b.hasAdvancedConfig = true
	return b
}

// WithMode sets the Zenoh session mode
func (b *ContextBuilder) WithMode(mode ZenohMode) *ContextBuilder {
	b.mode = mode
	b.hasAdvancedConfig = true
	return b
}

// ConnectToLocalZenohd connects to a local zenohd on tcp/127.0.0.1:7447
func (b *ContextBuilder) ConnectToLocalZenohd() *ContextBuilder {
	b.connectToLocalZenohd = true
	b.hasAdvancedConfig = true
	return b
}

// WithJSON passes arbitrary Zenoh configuration as a JSON string.
// The string must be a JSON object with dotted keys.
// Example: `{"scouting/multicast/enabled": false}`
func (b *ContextBuilder) WithJSON(jsonStr string) *ContextBuilder {
	b.jsonConfig = jsonStr
	b.hasAdvancedConfig = true
	return b
}

// WithRemapRules adds multiple name remapping rules
func (b *ContextBuilder) WithRemapRules(rules ...string) *ContextBuilder {
	b.remapRules = append(b.remapRules, rules...)
	b.hasAdvancedConfig = true
	return b
}

// WithLogging enables Zenoh logging initialization
func (b *ContextBuilder) WithLogging() *ContextBuilder {
	b.enableLogging = true
	b.hasAdvancedConfig = true
	return b
}

// Build creates the context
func (b *ContextBuilder) Build() (*Context, error) {
	var handle *C.ros_z_context_t

	if !b.hasAdvancedConfig {
		// Fast path: simple domain_id only
		handle = C.ros_z_context_create(C.uint32_t(b.domainID))
	} else {
		// Full config path
		pinner := &runtime.Pinner{}
		defer pinner.Unpin()

		var cfg C.ros_z_context_config_t
		cfg.domain_id = C.uint32_t(b.domainID)
		cfg.disable_multicast_scouting = C.bool(b.disableMulticast)
		cfg.connect_to_local_zenohd = C.bool(b.connectToLocalZenohd)
		cfg.enable_logging = C.bool(b.enableLogging)

		// Config file
		if b.configFile != "" {
			cPath := C.CString(b.configFile)
			defer C.free(unsafe.Pointer(cPath))
			cfg.config_file = cPath
		}

		// Connect endpoints
		if len(b.connectEndpoints) > 0 {
			cEndpoints := make([]*C.char, len(b.connectEndpoints))
			for i, ep := range b.connectEndpoints {
				cEndpoints[i] = C.CString(ep)
				defer C.free(unsafe.Pointer(cEndpoints[i]))
			}
			pinner.Pin(&cEndpoints[0])
			cfg.connect_endpoints = &cEndpoints[0]
			cfg.connect_endpoints_count = C.uintptr_t(len(b.connectEndpoints))
		}

		// Mode
		if b.mode != "" {
			cMode := C.CString(string(b.mode))
			defer C.free(unsafe.Pointer(cMode))
			cfg.mode = cMode
		}

		// JSON config
		if b.jsonConfig != "" {
			cJSON := C.CString(b.jsonConfig)
			defer C.free(unsafe.Pointer(cJSON))
			cfg.json_config = cJSON
		}

		// Remap rules
		if len(b.remapRules) > 0 {
			cRules := make([]*C.char, len(b.remapRules))
			for i, rule := range b.remapRules {
				cRules[i] = C.CString(rule)
				defer C.free(unsafe.Pointer(cRules[i]))
			}
			pinner.Pin(&cRules[0])
			cfg.remap_rules = &cRules[0]
			cfg.remap_rules_count = C.uintptr_t(len(b.remapRules))
		}

		handle = C.ros_z_context_create_with_config(&cfg)
	}

	if handle == nil {
		return nil, fmt.Errorf("%w: context", ErrBuildFailed)
	}

	ctx := &Context{handle: handle}
	runtime.SetFinalizer(ctx, (*Context).Close)

	return ctx, nil
}

// CreateNode creates a new node builder
func (c *Context) CreateNode(name string) *NodeBuilder {
	return &NodeBuilder{
		ctx:       c,
		name:      name,
		namespace: "",
	}
}

// Close shuts down the context
func (c *Context) Close() error {
	var err error
	c.closeOnce.Do(func() {
		if c.handle == nil {
			return
		}
		result := C.ros_z_context_destroy(c.handle)
		c.handle = nil
		if result != 0 {
			err = fmt.Errorf("context shutdown failed with code %d", result)
		}
	})
	return err
}
