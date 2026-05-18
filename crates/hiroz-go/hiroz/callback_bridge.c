#include "_cgo_export.h"
#include "hiroz_ffi.h"

hiroz_ServiceCallback getServiceCallback() {
	return (hiroz_ServiceCallback)goServiceCallback;
}

hiroz_ActionGoalCallback getActionGoalCallback() {
	return (hiroz_ActionGoalCallback)goActionGoalCallback;
}

hiroz_ActionExecuteCallback getActionExecuteCallback() {
	return (hiroz_ActionExecuteCallback)goActionExecuteCallback;
}

hiroz_MessageCallback getSubscriberCallback() {
	return (hiroz_MessageCallback)goSubscriberCallback;
}
