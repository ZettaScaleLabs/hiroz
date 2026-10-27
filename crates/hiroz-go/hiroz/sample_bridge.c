#include "_cgo_export.h"
#include "hiroz_ffi.h"
hiroz_SampleCallback getSampleCallback(void) { return (hiroz_SampleCallback)goSampleCallback; }
hiroz_SubscriberDropCallback getSampleDropCallback(void) { return (hiroz_SubscriberDropCallback)goSampleDropCallback; }
