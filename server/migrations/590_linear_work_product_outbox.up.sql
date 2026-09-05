CREATE FUNCTION enqueue_linear_work_product_outbox() RETURNS trigger AS $$
DECLARE
    binding RECORD;
BEGIN
    IF NEW.issue_id IS NULL OR NEW.detached_at IS NOT NULL THEN RETURN NEW; END IF;
    FOR binding IN
        SELECT b.id FROM linear_project_binding b
        JOIN issue i ON i.project_id=b.patchbay_project_id AND i.workspace_id=b.workspace_id
        WHERE i.id=NEW.issue_id AND i.workspace_id=NEW.workspace_id
          AND b.status='active' AND b.sync_mode IN ('publish','two_way')
    LOOP
        INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload)
        VALUES(gen_random_uuid(),NEW.workspace_id,binding.id,NEW.issue_id,
            'work-product:'||NEW.id::text||':attached','issue_updated','{}')
        ON CONFLICT(binding_id,event_key) DO NOTHING;
    END LOOP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER trg_linear_work_product_outbox AFTER INSERT OR UPDATE ON work_product_relation
FOR EACH ROW EXECUTE FUNCTION enqueue_linear_work_product_outbox();
