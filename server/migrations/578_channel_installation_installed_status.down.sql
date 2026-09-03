ALTER TABLE channel_installation DROP CONSTRAINT channel_installation_status_check;
UPDATE channel_installation SET status = 'active' WHERE status = 'installed';
ALTER TABLE channel_installation ALTER COLUMN status SET DEFAULT 'active';
ALTER TABLE channel_installation ADD CONSTRAINT channel_installation_status_check
    CHECK (status IN ('active', 'revoked'));
