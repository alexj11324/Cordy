-- Keep published migration history immutable and move the remaining persisted
-- product identifiers forward at one explicit compatibility boundary. The
-- old identity columns remain for one rolling-deployment compatibility window;
-- a bridge trigger lets old and new binaries write either spelling safely.

ALTER TABLE lark_user_binding
    ADD COLUMN patchbay_user_id UUID;

ALTER TABLE channel_user_binding
    ADD COLUMN patchbay_user_id UUID;

UPDATE lark_user_binding
SET patchbay_user_id = cordy_user_id; -- legacy-brand-compat

UPDATE channel_user_binding
SET patchbay_user_id = cordy_user_id; -- legacy-brand-compat

ALTER TABLE lark_user_binding
    ALTER COLUMN patchbay_user_id SET NOT NULL;

ALTER TABLE channel_user_binding
    ALTER COLUMN patchbay_user_id SET NOT NULL;

CREATE FUNCTION sync_patchbay_user_id_columns()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.patchbay_user_id IS DISTINCT FROM OLD.patchbay_user_id
           AND NEW.cordy_user_id IS NOT DISTINCT FROM OLD.cordy_user_id THEN -- legacy-brand-compat
            NEW.cordy_user_id := NEW.patchbay_user_id; -- legacy-brand-compat
        ELSIF NEW.cordy_user_id IS DISTINCT FROM OLD.cordy_user_id -- legacy-brand-compat
              AND NEW.patchbay_user_id IS NOT DISTINCT FROM OLD.patchbay_user_id THEN
            NEW.patchbay_user_id := NEW.cordy_user_id; -- legacy-brand-compat
        END IF;
    ELSE
        NEW.patchbay_user_id := COALESCE(NEW.patchbay_user_id, NEW.cordy_user_id); -- legacy-brand-compat
        NEW.cordy_user_id := COALESCE(NEW.cordy_user_id, NEW.patchbay_user_id); -- legacy-brand-compat
    END IF;

    IF NEW.patchbay_user_id IS NULL OR NEW.cordy_user_id IS NULL -- legacy-brand-compat
       OR NEW.patchbay_user_id IS DISTINCT FROM NEW.cordy_user_id THEN -- legacy-brand-compat
        RAISE EXCEPTION 'Patchbay user identity columns must agree';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER trg_lark_user_binding_brand_identity
BEFORE INSERT OR UPDATE OF patchbay_user_id, cordy_user_id ON lark_user_binding -- legacy-brand-compat
FOR EACH ROW
EXECUTE FUNCTION sync_patchbay_user_id_columns();

CREATE TRIGGER trg_channel_user_binding_brand_identity
BEFORE INSERT OR UPDATE OF patchbay_user_id, cordy_user_id ON channel_user_binding -- legacy-brand-compat
FOR EACH ROW
EXECUTE FUNCTION sync_patchbay_user_id_columns();

UPDATE agent_runtime
SET provider = 'patchbay_agent'
WHERE provider = 'cordy_agent';

DROP TRIGGER IF EXISTS trg_atq_dirty_hourly ON agent_task_queue;
CREATE TRIGGER trg_atq_dirty_hourly
BEFORE UPDATE OF runtime_id, issue_id OR DELETE ON agent_task_queue
FOR EACH ROW
WHEN (current_setting('patchbay.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_atq();

DROP TRIGGER IF EXISTS trg_issue_delete_dirty_hourly ON issue;
CREATE TRIGGER trg_issue_delete_dirty_hourly
BEFORE DELETE ON issue
FOR EACH ROW
WHEN (current_setting('patchbay.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_issue_delete();

DROP TRIGGER IF EXISTS trg_tu_dirty_hourly ON task_usage;
CREATE TRIGGER trg_tu_dirty_hourly
BEFORE DELETE ON task_usage
FOR EACH ROW
WHEN (current_setting('patchbay.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_tu();
