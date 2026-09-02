-- Single-statement concurrent build by repository rule.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS channel_installation_runtime_observation_uidx
    ON channel_installation_runtime_observation(installation_id);
