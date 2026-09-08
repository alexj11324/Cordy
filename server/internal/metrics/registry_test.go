package metrics

import "testing"

func TestRegistryExcludesDatabaseSampledMetrics(t *testing.T) {
	registry := NewRegistry(RegistryOptions{})
	families, err := registry.Gatherer.Gather()
	if err != nil {
		t.Fatalf("gather metrics: %v", err)
	}

	retired := map[string]struct{}{
		"orvilo_agent_task_queued":                               {},
		"orvilo_agent_task_running":                              {},
		"orvilo_agent_task_stuck_total":                          {},
		"orvilo_business_sampler_query_errors_total":             {},
		"orvilo_business_sampler_query_seconds":                  {},
		"orvilo_workspace_total":                                 {},
		"orvilo_seat_capacity_outbox_pending":                    {},
		"orvilo_seat_capacity_outbox_dead_lettered":              {},
		"orvilo_seat_capacity_outbox_oldest_pending_age_seconds": {},
		"orvilo_channel_media_pending_objects":                   {},
		"orvilo_channel_media_tombstoned_objects":                {},
		"orvilo_runtime_gc_blocked_observation_failed_total":     {},
		"orvilo_runtime_gc_blocked_runtimes":                     {},
		"orvilo_runtime_gc_backlog_runtimes":                     {},
	}
	for _, family := range families {
		if _, found := retired[family.GetName()]; found {
			t.Errorf("retired database-sampled metric %q is still registered", family.GetName())
		}
	}
}
