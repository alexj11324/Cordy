-- Installation existence is not connection liveness. Preserve every record,
-- credential, binding, pause and timestamp while giving that state its name.
ALTER TABLE channel_installation DROP CONSTRAINT channel_installation_status_check;
UPDATE channel_installation SET status = 'installed' WHERE status = 'active';
ALTER TABLE channel_installation ALTER COLUMN status SET DEFAULT 'installed';
ALTER TABLE channel_installation ADD CONSTRAINT channel_installation_status_check
    CHECK (status IN ('installed', 'revoked'));
