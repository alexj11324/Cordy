CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_automation_trigger_native_event
    ON automation_trigger (provider, preset)
    WHERE kind = 'webhook' AND enabled AND preset IS NOT NULL;
