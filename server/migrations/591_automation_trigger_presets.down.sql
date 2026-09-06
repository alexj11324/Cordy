-- Native trigger rows and their fanned-out deliveries have no representation
-- in the pre-591 schema. Remove them before restoring the old provider checks;
-- otherwise a rollback fails after the feature has been used because `slack`
-- and `linear` are not accepted by those checks.
DELETE FROM webhook_delivery WHERE provider IN ('slack', 'linear');
DELETE FROM automation_trigger WHERE provider IN ('slack', 'linear');

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
