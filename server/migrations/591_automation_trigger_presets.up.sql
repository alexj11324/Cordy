-- Cursor-style trigger presets: a webhook/schedule row can name a catalog
-- event (github.pull_request.opened, slack.message, …) plus card-level config.
-- Native GitHub/Slack/Linear triggers keep kind=webhook but do not mint a
-- public URL; generic webhook triggered still does.
--
-- Provider CHECK widens from generic|github to include slack and linear.
-- Existing github+token rows stay valid for the public ingress.

ALTER TABLE automation_trigger
    DROP CONSTRAINT IF EXISTS automation_trigger_provider_check;

ALTER TABLE automation_trigger
    ADD CONSTRAINT automation_trigger_provider_check
    CHECK (provider IN ('generic', 'github', 'slack', 'linear'));

ALTER TABLE automation_trigger
    ADD COLUMN IF NOT EXISTS preset TEXT,
    ADD COLUMN IF NOT EXISTS config JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE webhook_delivery
    DROP CONSTRAINT IF EXISTS webhook_delivery_provider_check;

ALTER TABLE webhook_delivery
    ADD CONSTRAINT webhook_delivery_provider_check
    CHECK (provider IN ('generic', 'github', 'slack', 'linear'));
