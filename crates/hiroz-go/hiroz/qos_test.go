package hiroz

import "testing"

func TestQosDefault(t *testing.T) {
	qos := QosDefault()
	if qos.Reliability != ReliabilityReliable {
		t.Errorf("expected Reliable, got %d", qos.Reliability)
	}
	if qos.Durability != DurabilityVolatile {
		t.Errorf("expected Volatile, got %d", qos.Durability)
	}
	if qos.History != HistoryKeepLast {
		t.Errorf("expected KeepLast, got %d", qos.History)
	}
	if qos.HistoryDepth != 10 {
		t.Errorf("expected depth 10, got %d", qos.HistoryDepth)
	}
	if qos.Liveliness != LivelinessAutomatic {
		t.Errorf("expected Automatic, got %d", qos.Liveliness)
	}
	if qos.Deadline != QosDurationInfinite() {
		t.Errorf("expected infinite deadline")
	}
	if qos.Lifespan != QosDurationInfinite() {
		t.Errorf("expected infinite lifespan")
	}
}

func TestQosSensorData(t *testing.T) {
	qos := QosSensorData()
	if qos.Reliability != ReliabilityBestEffort {
		t.Errorf("expected BestEffort, got %d", qos.Reliability)
	}
	if qos.HistoryDepth != 5 {
		t.Errorf("expected depth 5, got %d", qos.HistoryDepth)
	}
}

func TestQosTransientLocal(t *testing.T) {
	qos := QosTransientLocal()
	if qos.Durability != DurabilityTransientLocal {
		t.Errorf("expected TransientLocal, got %d", qos.Durability)
	}
	if qos.HistoryDepth != 1 {
		t.Errorf("expected depth 1, got %d", qos.HistoryDepth)
	}
}

func TestQosKeepAll(t *testing.T) {
	qos := QosKeepAll()
	if qos.History != HistoryKeepAll {
		t.Errorf("expected KeepAll, got %d", qos.History)
	}
}

func TestQosParameterEvents(t *testing.T) {
	qos := QosParameterEvents()
	if qos.HistoryDepth != 1000 {
		t.Errorf("expected depth 1000, got %d", qos.HistoryDepth)
	}
}

func TestQosToCQos(t *testing.T) {
	qos := QosDefault()
	cQos := qos.toCQos()

	if int32(cQos.reliability) != int32(ReliabilityReliable) {
		t.Errorf("C reliability mismatch")
	}
	if int32(cQos.durability) != int32(DurabilityVolatile) {
		t.Errorf("C durability mismatch")
	}
	if int32(cQos.history) != int32(HistoryKeepLast) {
		t.Errorf("C history mismatch")
	}
	if int32(cQos.history_depth) != 10 {
		t.Errorf("C history_depth mismatch")
	}
}
