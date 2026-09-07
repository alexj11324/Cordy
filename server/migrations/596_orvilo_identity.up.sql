-- Rename remaining Patchbay schema identity to Orvilo. Historical
-- migration files stay byte-identical; this file is the live cutover.

ALTER TABLE linear_comment_link DROP CONSTRAINT IF EXISTS linear_comment_link_origin_check;
UPDATE linear_comment_link SET origin = 'orvilo' WHERE origin = 'patchbay';
ALTER TABLE linear_comment_link ADD CONSTRAINT linear_comment_link_origin_check
    CHECK (origin IN ('linear', 'orvilo'));

ALTER TABLE linear_project_binding DROP CONSTRAINT IF EXISTS linear_project_binding_initial_source_of_truth_check;
UPDATE linear_project_binding SET initial_source_of_truth = 'orvilo' WHERE initial_source_of_truth = 'patchbay';
ALTER TABLE linear_project_binding ADD CONSTRAINT linear_project_binding_initial_source_of_truth_check
    CHECK (initial_source_of_truth IS NULL OR initial_source_of_truth IN ('linear', 'orvilo'));

ALTER TABLE desktop_auth_handoff DROP CONSTRAINT IF EXISTS desktop_auth_handoff_protocol_check;
UPDATE desktop_auth_handoff SET callback_protocol = 'orvilo' WHERE callback_protocol = 'patchbay';
UPDATE desktop_auth_handoff SET callback_protocol = regexp_replace(callback_protocol, '^patchbay-canary-', 'orvilo-canary-')
    WHERE callback_protocol LIKE 'patchbay-canary-%';
ALTER TABLE desktop_auth_handoff ADD CONSTRAINT desktop_auth_handoff_protocol_check
    CHECK (callback_protocol ~ '^(orvilo|orvilo-canary-[a-f0-9]{16})$');

ALTER TABLE channel_user_binding RENAME COLUMN patchbay_user_id TO orvilo_user_id;
ALTER TABLE linear_member_binding RENAME COLUMN patchbay_user_id TO orvilo_user_id;
ALTER TABLE linear_project_binding RENAME COLUMN patchbay_project_id TO orvilo_project_id;
ALTER TABLE linear_issue_link RENAME COLUMN patchbay_issue_id TO orvilo_issue_id;
ALTER TABLE linear_sync_conflict RENAME COLUMN patchbay_issue_id TO orvilo_issue_id;
ALTER TABLE lark_user_binding RENAME COLUMN patchbay_user_id TO orvilo_user_id;

CREATE OR REPLACE FUNCTION enqueue_linear_issue_outbox() RETURNS trigger AS $$
DECLARE
    source_issue issue%ROWTYPE;
    operation TEXT;
    binding RECORD;
BEGIN
    IF current_setting('orvilo.linear_remote_apply', true) = 'on' THEN
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
    IF TG_OP = 'DELETE' THEN
        source_issue := OLD;
    ELSE
        source_issue := NEW;
    END IF;
    operation := CASE TG_OP WHEN 'INSERT' THEN 'issue_created' WHEN 'DELETE' THEN 'issue_deleted' ELSE 'issue_updated' END;
    FOR binding IN
        SELECT id FROM linear_project_binding
        WHERE workspace_id = source_issue.workspace_id
          AND orvilo_project_id = source_issue.project_id
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
        ) ON CONFLICT (binding_id, event_key) DO NOTHING;
    END LOOP;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;


CREATE OR REPLACE FUNCTION enqueue_linear_comment_outbox() RETURNS trigger AS $$
DECLARE
    source_comment comment%ROWTYPE;
    binding RECORD;
    operation TEXT;
BEGIN
    IF current_setting('orvilo.linear_remote_apply', true) = 'on' THEN
        RETURN COALESCE(NEW, OLD);
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.content IS NOT DISTINCT FROM OLD.content THEN RETURN NEW; END IF;
    source_comment := CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
    -- Platform bookkeeping is not user discussion. Imported comments also use
    -- system authorship, so they can never turn into outbound echoes.
    IF source_comment.author_type NOT IN ('member','agent') OR source_comment.type <> 'comment' THEN RETURN COALESCE(NEW, OLD); END IF;
    operation := CASE TG_OP WHEN 'INSERT' THEN 'comment_created' WHEN 'UPDATE' THEN 'comment_updated' ELSE 'comment_deleted' END;
    FOR binding IN
        SELECT b.id FROM linear_project_binding b
        JOIN issue i ON i.project_id=b.orvilo_project_id AND i.workspace_id=b.workspace_id
        WHERE i.id=source_comment.issue_id AND i.workspace_id=source_comment.workspace_id
          AND b.status='active' AND b.sync_mode IN ('publish','two_way')
    LOOP
        INSERT INTO linear_comment_link(workspace_id,binding_id,issue_id,comment_id,linear_comment_id,origin)
        VALUES(source_comment.workspace_id,binding.id,source_comment.issue_id,source_comment.id,gen_random_uuid()::text,'orvilo')
        ON CONFLICT(binding_id,comment_id) DO NOTHING;
        INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload)
        VALUES(gen_random_uuid(),source_comment.workspace_id,binding.id,source_comment.issue_id,
            'comment:'||source_comment.id::text||':'||operation||':'||source_comment.revision::text,
            operation,jsonb_build_object('comment_id',source_comment.id,'body',source_comment.content,
                'parent_id',source_comment.parent_id,'author_type',source_comment.author_type,'author_id',source_comment.author_id))
        ON CONFLICT(binding_id,event_key) DO NOTHING;
    END LOOP;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;


CREATE OR REPLACE FUNCTION enqueue_linear_work_product_outbox() RETURNS trigger AS $$
DECLARE
    binding RECORD;
    product RECORD;
    operation TEXT;
    event_version TEXT := '';
    source_relation work_product_relation%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN source_relation := OLD; ELSE source_relation := NEW; END IF;
    IF source_relation.issue_id IS NULL THEN RETURN COALESCE(NEW, OLD); END IF;
    IF TG_OP='UPDATE'
       AND OLD.relation_source IN ('manual_explicit','task_explicit','execution_branch_discovery','provider_discovery')
       AND NEW.relation_source NOT IN ('manual_explicit','task_explicit','execution_branch_discovery','provider_discovery') THEN
        source_relation := OLD;
        operation := 'attachment_deleted';
    ELSIF source_relation.relation_source NOT IN ('manual_explicit','task_explicit','execution_branch_discovery','provider_discovery') THEN
        RETURN COALESCE(NEW, OLD);
    END IF;
    IF TG_OP='UPDATE' AND OLD.relation_source IS DISTINCT FROM NEW.relation_source THEN
        event_version := ':'||gen_random_uuid()::text;
    END IF;
    SELECT kind, external_url INTO product FROM work_product
    WHERE id=source_relation.work_product_id AND workspace_id=source_relation.workspace_id;
    IF product.kind <> 'pull_request' OR COALESCE(product.external_url, '') = '' THEN RETURN COALESCE(NEW, OLD); END IF;
    IF operation IS NOT NULL THEN
        NULL;
    ELSIF TG_OP = 'DELETE' THEN
        operation := 'attachment_deleted';
    ELSIF TG_OP = 'UPDATE' AND OLD.detached_at IS NULL AND NEW.detached_at IS NOT NULL THEN
        operation := 'attachment_deleted';
    ELSIF NEW.detached_at IS NULL THEN
        operation := 'issue_updated';
    ELSE
        RETURN COALESCE(NEW, OLD);
    END IF;
    IF operation='attachment_deleted' AND EXISTS (
        SELECT 1 FROM work_product_relation other
        WHERE other.workspace_id=source_relation.workspace_id
          AND other.work_product_id=source_relation.work_product_id
          AND other.issue_id=source_relation.issue_id
          AND other.id<>source_relation.id
          AND other.detached_at IS NULL
          AND other.relation_source IN ('manual_explicit','task_explicit','execution_branch_discovery','provider_discovery')
    ) THEN RETURN COALESCE(NEW, OLD); END IF;
    FOR binding IN
        SELECT b.id FROM linear_project_binding b
        JOIN issue i ON i.project_id=b.orvilo_project_id AND i.workspace_id=b.workspace_id
        WHERE i.id=source_relation.issue_id AND i.workspace_id=source_relation.workspace_id
          AND b.status='active' AND b.sync_mode IN ('publish','two_way')
    LOOP
        INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload)
        VALUES(gen_random_uuid(),source_relation.workspace_id,binding.id,source_relation.issue_id,
            'work-product:'||source_relation.id::text||':'||operation||event_version,operation,
            CASE WHEN operation='attachment_deleted' THEN jsonb_build_object('url',product.external_url) ELSE '{}'::jsonb END)
        ON CONFLICT(binding_id,event_key) DO NOTHING;
    END LOOP;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION enqueue_linear_work_product_url_outbox() RETURNS trigger AS $$
DECLARE
    target RECORD;
BEGIN
    IF NEW.external_url IS NOT DISTINCT FROM OLD.external_url THEN RETURN NEW; END IF;
    FOR target IN
        SELECT r.id AS relation_id,r.issue_id,b.id AS binding_id
        FROM work_product_relation r
        JOIN issue i ON i.id=r.issue_id AND i.workspace_id=r.workspace_id
        JOIN linear_project_binding b ON b.orvilo_project_id=i.project_id AND b.workspace_id=i.workspace_id
        WHERE r.work_product_id=NEW.id AND r.workspace_id=NEW.workspace_id
          AND r.detached_at IS NULL AND r.issue_id IS NOT NULL
          AND b.status='active' AND b.sync_mode IN ('publish','two_way')
    LOOP
        IF COALESCE(OLD.external_url, '') <> '' THEN
            INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload)
            VALUES(gen_random_uuid(),NEW.workspace_id,target.binding_id,target.issue_id,
                'work-product:'||target.relation_id::text||':url-delete:'||md5(OLD.external_url)||':'||extract(epoch FROM NEW.updated_at)::text,
                'attachment_deleted',jsonb_build_object('url',OLD.external_url))
            ON CONFLICT(binding_id,event_key) DO NOTHING;
        END IF;
        IF COALESCE(NEW.external_url, '') <> '' THEN
            INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload)
            VALUES(gen_random_uuid(),NEW.workspace_id,target.binding_id,target.issue_id,
                'work-product:'||target.relation_id::text||':url-upsert:'||md5(NEW.external_url)||':'||extract(epoch FROM NEW.updated_at)::text,
                'issue_updated','{}')
            ON CONFLICT(binding_id,event_key) DO NOTHING;
        END IF;
    END LOOP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Workspace teardown removes the raw and rolled-up usage for the entire
-- workspace, so per-row dirty-key maintenance is pure overhead in that one
-- transaction. Keep normal deletes unchanged while allowing the application
-- deletion plan to opt out with a transaction-local setting.

DROP TRIGGER IF EXISTS trg_atq_dirty_hourly ON agent_task_queue;
CREATE TRIGGER trg_atq_dirty_hourly
BEFORE UPDATE OF runtime_id, issue_id OR DELETE ON agent_task_queue
FOR EACH ROW
WHEN (current_setting('orvilo.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_atq();

DROP TRIGGER IF EXISTS trg_issue_delete_dirty_hourly ON issue;
CREATE TRIGGER trg_issue_delete_dirty_hourly
BEFORE DELETE ON issue
FOR EACH ROW
WHEN (current_setting('orvilo.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_issue_delete();

DROP TRIGGER IF EXISTS trg_tu_dirty_hourly ON task_usage;
CREATE TRIGGER trg_tu_dirty_hourly
BEFORE DELETE ON task_usage
FOR EACH ROW
WHEN (current_setting('orvilo.workspace_teardown', true) IS DISTINCT FROM 'on')
EXECUTE FUNCTION enqueue_task_usage_hourly_dirty_for_tu();

