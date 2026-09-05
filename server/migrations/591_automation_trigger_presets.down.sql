ALTER TABLE webhook_delivery
    DROP CONSTRAINT IF EXISTS webhook_delivery_provider_check;

ALTER TABLE webhook_delivery
    ADD CONSTRAINT webhook_delivery_provider_check
    CHECK (provider IN ('generic', 'github'));

ALTER TABLE automation_trigger
    DROP COLUMN IF EXISTS config,
    DROP COLUMN IF EXISTS preset;

ALTER TABLE automation_trigger
    DROP CONSTRAINT IF EXISTS automation_trigger_provider_check;

ALTER TABLE automation_trigger
    ADD CONSTRAINT automation_trigger_provider_check
    CHECK (provider IN ('generic', 'github'));
