package rosz

/*
#include "ros_z_ffi.h"
*/
import "C"

// QosReliability controls message delivery guarantees
type QosReliability int32

const (
	// ReliabilityReliable retransmits lost messages (default)
	ReliabilityReliable QosReliability = 0
	// ReliabilityBestEffort delivers messages without retransmission
	ReliabilityBestEffort QosReliability = 1
)

// QosDurability controls whether late-joining subscribers see past messages
type QosDurability int32

const (
	// DurabilityVolatile only delivers messages published after subscription (default)
	DurabilityVolatile QosDurability = 0
	// DurabilityTransientLocal delivers cached messages to late-joining subscribers
	DurabilityTransientLocal QosDurability = 1
)

// QosHistory controls how many messages are kept
type QosHistory int32

const (
	// HistoryKeepLast keeps only the last N messages (default)
	HistoryKeepLast QosHistory = 0
	// HistoryKeepAll keeps all messages (limited by system resources)
	HistoryKeepAll QosHistory = 1
)

// QosLiveliness controls how liveliness is asserted
type QosLiveliness int32

const (
	// LivelinessAutomatic asserts liveliness automatically (default)
	LivelinessAutomatic QosLiveliness = 0
	// LivelinessManualByNode requires manual liveliness assertion per node
	LivelinessManualByNode QosLiveliness = 1
	// LivelinessManualByTopic requires manual liveliness assertion per topic
	LivelinessManualByTopic QosLiveliness = 2
)

// QosDuration represents a duration in seconds and nanoseconds for QoS settings
type QosDuration struct {
	Sec  uint64
	Nsec uint64
}

// QosDurationInfinite returns an infinite duration (the default for QoS time constraints).
//
// The encoding follows the ROS 2 / DDS convention for "infinite" durations:
// INT64_MAX nanoseconds split into seconds and nanoseconds components.
//
//	INT64_MAX = 9_223_372_036_854_775_807 ns
//	         = 9_223_372_036 s  +  854_775_807 ns
//
// Both rmw_zenoh_cpp and rclcpp treat this specific value as "no deadline" /
// "no lifespan" / "infinite lease". Do not substitute arbitrary large values.
func QosDurationInfinite() QosDuration {
	return QosDuration{Sec: 9223372036, Nsec: 854775807}
}

// QosProfile contains all QoS settings for a publisher or subscriber
type QosProfile struct {
	Reliability             QosReliability
	Durability              QosDurability
	History                 QosHistory
	HistoryDepth            int
	Deadline                QosDuration
	Lifespan                QosDuration
	Liveliness              QosLiveliness
	LivelinessLeaseDuration QosDuration
}

// QosDefault returns the default QoS profile (Reliable, Volatile, KeepLast(10))
func QosDefault() QosProfile {
	return QosProfile{
		Reliability:             ReliabilityReliable,
		Durability:              DurabilityVolatile,
		History:                 HistoryKeepLast,
		HistoryDepth:            10,
		Deadline:                QosDurationInfinite(),
		Lifespan:                QosDurationInfinite(),
		Liveliness:              LivelinessAutomatic,
		LivelinessLeaseDuration: QosDurationInfinite(),
	}
}

// QosSensorData returns QoS suitable for sensor data (BestEffort, Volatile, KeepLast(5))
func QosSensorData() QosProfile {
	qos := QosDefault()
	qos.Reliability = ReliabilityBestEffort
	qos.HistoryDepth = 5
	return qos
}

// QosServicesDefault returns QoS suitable for services (Reliable, Volatile, KeepLast(10))
func QosServicesDefault() QosProfile {
	return QosDefault()
}

// QosParameterEvents returns QoS for parameter events (Reliable, Volatile, KeepLast(1000))
func QosParameterEvents() QosProfile {
	qos := QosDefault()
	qos.HistoryDepth = 1000
	return qos
}

// QosKeepAll returns QoS that keeps all messages (Reliable, Volatile, KeepAll)
func QosKeepAll() QosProfile {
	qos := QosDefault()
	qos.History = HistoryKeepAll
	return qos
}

// QosTransientLocal returns QoS with transient local durability (Reliable, TransientLocal, KeepLast(1))
// Suitable for /robot_description, /tf_static
func QosTransientLocal() QosProfile {
	qos := QosDefault()
	qos.Durability = DurabilityTransientLocal
	qos.HistoryDepth = 1
	return qos
}

// toCQos converts a QosProfile to a C-compatible struct
func (q *QosProfile) toCQos() C.ros_z_qos_profile_t {
	return C.ros_z_qos_profile_t{
		reliability:           C.int32_t(q.Reliability),
		durability:            C.int32_t(q.Durability),
		history:               C.int32_t(q.History),
		history_depth:         C.int32_t(q.HistoryDepth),
		deadline_sec:          C.uint64_t(q.Deadline.Sec),
		deadline_nsec:         C.uint64_t(q.Deadline.Nsec),
		lifespan_sec:          C.uint64_t(q.Lifespan.Sec),
		lifespan_nsec:         C.uint64_t(q.Lifespan.Nsec),
		liveliness:            C.int32_t(q.Liveliness),
		liveliness_lease_sec:  C.uint64_t(q.LivelinessLeaseDuration.Sec),
		liveliness_lease_nsec: C.uint64_t(q.LivelinessLeaseDuration.Nsec),
	}
}
