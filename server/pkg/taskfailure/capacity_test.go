package taskfailure

import "testing"

func TestSelectedModelCapacity(t *testing.T) {
	if got := Classify("Selected model is at capacity. Please try a different model."); got != ReasonAgentProviderCapacityOrRateLimit {
		t.Fatalf("got %s, want capacity", got)
	}
}
