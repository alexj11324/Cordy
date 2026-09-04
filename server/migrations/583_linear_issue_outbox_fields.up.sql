-- Keep Linear outbound durable for every shared field. This is a replacement
-- migration because 559 may already be applied in a deployed database.
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
       NEW.executor_id IS NOT DISTINCT FROM OLD.executor_id AND
       NEW.due_date IS NOT DISTINCT FROM OLD.due_date AND
       NEW.owner_type IS NOT DISTINCT FROM OLD.owner_type AND
       NEW.owner_id IS NOT DISTINCT FROM OLD.owner_id THEN
        RETURN NEW;
    END IF;
    source_issue := CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
    operation := CASE TG_OP WHEN 'INSERT' THEN 'issue_created' WHEN 'DELETE' THEN 'issue_deleted' ELSE 'issue_updated' END;
    FOR binding IN
        SELECT id FROM linear_project_binding
        WHERE workspace_id = source_issue.workspace_id
          AND patchbay_project_id = source_issue.project_id
          AND status = 'active'
          AND sync_mode IN ('publish', 'two_way')
    LOOP
        INSERT INTO linear_sync_outbox
            (id, workspace_id, binding_id, issue_id, event_key, event_type, payload)
        VALUES
            (gen_random_uuid(), source_issue.workspace_id, binding.id, source_issue.id,
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
                 'due_date', source_issue.due_date,
                 'owner_type', source_issue.owner_type,
                 'owner_id', source_issue.owner_id,
                 'revision', source_issue.revision
             )
        ON CONFLICT (binding_id, event_key) DO NOTHING;
    END LOOP;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;
