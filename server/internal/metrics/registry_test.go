package metrics

import "testing"

func TestRegistryExcludesDatabaseSampledMetrics(t *testing.T) {
	registry := NewRegistry(RegistryOptions{})
	families, err := registry.Gatherer.Gather()
	if err != nil {
		t.Fatalf("gather metrics: %v", err)
	}

	retired := map[string]struct{}{
		"patchbay_agent_task_queued":                               {},
		"patchbay_agent_task_running":                              {},
		"patchbay_agent_task_stuck_total":                          {},
		"patchbay_business_sampler_query_errors_total":             {},
		"patchbay_business_sampler_query_seconds":                  {},
		"patchbay_workspace_total":                                 {},
		"patchbay_seat_capacity_outbox_pending":                    {},
		"patchbay_seat_capacity_outbox_dead_lettered":              {},
		"patchbay_seat_capacity_outbox_oldest_pending_age_seconds": {},
		"patchbay_channel_media_pending_objects":                   {},
		"patchbay_channel_media_tombstoned_objects":                {},
		"patchbay_runtime_gc_blocked_observation_failed_total":     {},
		"patchbay_runtime_gc_blocked_runtimes":                     {},
		"patchbay_runtime_gc_backlog_runtimes":                     {},
	}
	for _, family := range families {
		if _, found := retired[family.GetName()]; found {
			t.Errorf("retired database-sampled metric %q is still registered", family.GetName())
		}
	}
}
