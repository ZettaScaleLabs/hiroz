#include "_cgo_export.h"
#include "ros_z_ffi.h"

ros_z_ServiceCallback getServiceCallback() {
	return (ros_z_ServiceCallback)goServiceCallback;
}

ros_z_ActionGoalCallback getActionGoalCallback() {
	return (ros_z_ActionGoalCallback)goActionGoalCallback;
}

ros_z_ActionExecuteCallback getActionExecuteCallback() {
	return (ros_z_ActionExecuteCallback)goActionExecuteCallback;
}

ros_z_MessageCallback getSubscriberCallback() {
	return (ros_z_MessageCallback)goSubscriberCallback;
}
