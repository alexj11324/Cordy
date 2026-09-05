CREATE FUNCTION enqueue_linear_work_product_outbox() RETURNS trigger AS $$
DECLARE
    binding RECORD;
    product RECORD;
    operation TEXT;
    source_relation work_product_relation%ROWTYPE;
BEGIN
    IF TG_OP = 'DELETE' THEN source_relation := OLD; ELSE source_relation := NEW; END IF;
    IF source_relation.issue_id IS NULL THEN RETURN COALESCE(NEW, OLD); END IF;
    IF source_relation.relation_source NOT IN ('manual_explicit','task_explicit','execution_branch_discovery','provider_discovery') THEN RETURN COALESCE(NEW, OLD); END IF;
    SELECT kind, external_url INTO product FROM work_product
    WHERE id=source_relation.work_product_id AND workspace_id=source_relation.workspace_id;
    IF product.kind <> 'pull_request' OR COALESCE(product.external_url, '') = '' THEN RETURN COALESCE(NEW, OLD); END IF;
    IF TG_OP = 'DELETE' THEN
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
        JOIN issue i ON i.project_id=b.patchbay_project_id AND i.workspace_id=b.workspace_id
        WHERE i.id=source_relation.issue_id AND i.workspace_id=source_relation.workspace_id
          AND b.status='active' AND b.sync_mode IN ('publish','two_way')
    LOOP
        INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload)
        VALUES(gen_random_uuid(),source_relation.workspace_id,binding.id,source_relation.issue_id,
            'work-product:'||source_relation.id::text||':'||operation,operation,
            CASE WHEN operation='attachment_deleted' THEN jsonb_build_object('url',product.external_url) ELSE '{}'::jsonb END)
        ON CONFLICT(binding_id,event_key) DO NOTHING;
    END LOOP;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER trg_linear_work_product_outbox BEFORE INSERT OR UPDATE OR DELETE ON work_product_relation
FOR EACH ROW EXECUTE FUNCTION enqueue_linear_work_product_outbox();

CREATE FUNCTION enqueue_linear_work_product_url_outbox() RETURNS trigger AS $$
DECLARE
    target RECORD;
BEGIN
    IF NEW.external_url IS NOT DISTINCT FROM OLD.external_url THEN RETURN NEW; END IF;
    FOR target IN
        SELECT r.id AS relation_id,r.issue_id,b.id AS binding_id
        FROM work_product_relation r
        JOIN issue i ON i.id=r.issue_id AND i.workspace_id=r.workspace_id
        JOIN linear_project_binding b ON b.patchbay_project_id=i.project_id AND b.workspace_id=i.workspace_id
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
CREATE TRIGGER trg_linear_work_product_url_outbox AFTER UPDATE OF external_url ON work_product
FOR EACH ROW EXECUTE FUNCTION enqueue_linear_work_product_url_outbox();
