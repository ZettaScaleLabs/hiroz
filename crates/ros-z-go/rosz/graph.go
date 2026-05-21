package rosz

/*
#include <stdlib.h>
#include "ros_z_ffi.h"
*/
import "C"
import (
	"fmt"
	"unsafe"
)

// TopicInfo describes a discovered topic
type TopicInfo struct {
	Name     string
	TypeName string
}

// NodeInfo describes a discovered node
type NodeInfo struct {
	Name      string
	Namespace string
}

// ServiceInfo describes a discovered service
type ServiceInfo struct {
	Name     string
	TypeName string
}

// GetTopicNamesAndTypes returns all topics visible in the ROS graph
func (c *Context) GetTopicNamesAndTypes() ([]TopicInfo, error) {
	if c.handle == nil {
		return nil, fmt.Errorf("context is closed")
	}

	var cTopics *C.ros_z_topic_info_t
	var count C.uintptr_t

	result := C.ros_z_graph_get_topic_names_and_types(c.handle, &cTopics, &count)
	if result != 0 {
		return nil, newRoszError(ErrorCode(result), "failed to get topic names and types")
	}

	n := int(count)
	if n == 0 {
		return []TopicInfo{}, nil
	}
	defer C.ros_z_graph_free_topics(cTopics, count)

	topics := make([]TopicInfo, n)
	cSlice := unsafe.Slice(cTopics, n)
	for i := 0; i < n; i++ {
		topics[i] = TopicInfo{
			Name:     C.GoString(cSlice[i].name),
			TypeName: C.GoString(cSlice[i].type_name),
		}
	}

	return topics, nil
}

// GetNodeNames returns all nodes visible in the ROS graph
func (c *Context) GetNodeNames() ([]NodeInfo, error) {
	if c.handle == nil {
		return nil, fmt.Errorf("context is closed")
	}

	var cNodes *C.ros_z_node_info_t
	var count C.uintptr_t

	result := C.ros_z_graph_get_node_names(c.handle, &cNodes, &count)
	if result != 0 {
		return nil, newRoszError(ErrorCode(result), "failed to get node names")
	}

	n := int(count)
	if n == 0 {
		return []NodeInfo{}, nil
	}
	defer C.ros_z_graph_free_nodes(cNodes, count)

	nodes := make([]NodeInfo, n)
	cSlice := unsafe.Slice(cNodes, n)
	for i := 0; i < n; i++ {
		nodes[i] = NodeInfo{
			Name:      C.GoString(cSlice[i].name),
			Namespace: C.GoString(cSlice[i].namespace_),
		}
	}

	return nodes, nil
}

// GetServiceNamesAndTypes returns all services visible in the ROS graph
func (c *Context) GetServiceNamesAndTypes() ([]ServiceInfo, error) {
	if c.handle == nil {
		return nil, fmt.Errorf("context is closed")
	}

	var cServices *C.ros_z_service_info_t
	var count C.uintptr_t

	result := C.ros_z_graph_get_service_names_and_types(c.handle, &cServices, &count)
	if result != 0 {
		return nil, newRoszError(ErrorCode(result), "failed to get service names and types")
	}

	n := int(count)
	if n == 0 {
		return []ServiceInfo{}, nil
	}
	defer C.ros_z_graph_free_services(cServices, count)

	services := make([]ServiceInfo, n)
	cSlice := unsafe.Slice(cServices, n)
	for i := 0; i < n; i++ {
		services[i] = ServiceInfo{
			Name:     C.GoString(cSlice[i].name),
			TypeName: C.GoString(cSlice[i].type_name),
		}
	}

	return services, nil
}

// NodeExists checks if a node with the given name and namespace exists in the graph
func (c *Context) NodeExists(name, namespace string) (bool, error) {
	if c.handle == nil {
		return false, fmt.Errorf("context is closed")
	}

	cName := C.CString(name)
	defer C.free(unsafe.Pointer(cName))

	cNamespace := C.CString(namespace)
	defer C.free(unsafe.Pointer(cNamespace))

	result := C.ros_z_graph_node_exists(c.handle, cName, cNamespace)
	if result < 0 {
		return false, newRoszError(ErrorCode(result), "failed to check node existence")
	}

	return result == 1, nil
}
