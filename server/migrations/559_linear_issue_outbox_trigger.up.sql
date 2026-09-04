-- Enqueue local issue mutations in the same transaction as the issue write.
-- Remote applies set a transaction-local marker so provider echoes do not
-- create a second outbound event.
ALTER TABLE linear_sync_outbox DROP CONSTRAINT IF EXISTS linear_sync_outbox_event_type_check;
ALTER TABLE linear_sync_outbox ADD CONSTRAINT linear_sync_outbox_event_type_check
    CHECK (event_type IN ('issue_created', 'issue_updated', 'issue_deleted'));

CREATE OR REPLACE FUNCTION enqueue_linear_issue_outbox() RETURNS trigger AS $$
DECLARE
    source_issue issue%ROWTYPE;
    operation TEXT;
    binding RECORD;
BEGIN
    IF current_setting('patchbay.linear_remote_apply', true) = 'on' THEN
        RETURN COALESCE(NEW, OLD);
    END IF;
    IF TG_OP = 'UPDATE' AND
       NEW.title IS NOT DISTINCT FROM OLD.title AND
       NEW.description IS NOT DISTINCT FROM OLD.description AND
       NEW.status IS NOT DISTINCT FROM OLD.status AND
       NEW.priority IS NOT DISTINCT FROM OLD.priority AND
       NEW.project_id IS NOT DISTINCT FROM OLD.project_id AND
       NEW.executor_id IS NOT DISTINCT FROM OLD.executor_id THEN
        RETURN NEW;
    END IF;
    IF TG_OP = 'DELETE' THEN
        source_issue := OLD;
    ELSE
        source_issue := NEW;
    END IF;
    operation := CASE TG_OP WHEN 'INSERT' THEN 'issue_created' WHEN 'DELETE' THEN 'issue_deleted' ELSE 'issue_updated' END;
    FOR binding IN
        SELECT id
        FROM linear_project_binding
        WHERE workspace_id = source_issue.workspace_id
          AND patchbay_project_id = source_issue.project_id
          AND status = 'active'
          AND sync_mode IN ('publish', 'two_way')
    LOOP
        INSERT INTO linear_sync_outbox (
            id, workspace_id, binding_id, issue_id, event_key, event_type, payload
        ) VALUES (
            gen_random_uuid(), source_issue.workspace_id, binding.id, source_issue.id,
            'issue:' || source_issue.id::text || ':' || operation || ':' || source_issue.revision::text,
            operation,
            jsonb_build_object(
                'id', source_issue.id,
                'title', source_issue.title,
                'description', source_issue.description,
                'status', source_issue.status,
                'priority', source_issue.priority,
                'project_id', source_issue.project_id,
                'executor_id', source_issue.executor_id,
                'revision', source_issue.revision
            )
        ) ON CONFLICT (binding_id, event_key) DO NOTHING;
    END LOOP;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_linear_issue_outbox
AFTER INSERT OR UPDATE OR DELETE ON issue
FOR EACH ROW EXECUTE FUNCTION enqueue_linear_issue_outbox();
