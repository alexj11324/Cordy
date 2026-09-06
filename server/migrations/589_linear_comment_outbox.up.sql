ALTER TABLE linear_sync_outbox DROP CONSTRAINT linear_sync_outbox_event_type_check;
ALTER TABLE linear_sync_outbox ADD CONSTRAINT linear_sync_outbox_event_type_check
    CHECK (event_type IN ('issue_created','issue_updated','issue_deleted','comment_created','comment_updated','comment_deleted','attachment_deleted'));

CREATE FUNCTION enqueue_linear_comment_outbox() RETURNS trigger AS $$
DECLARE
    source_comment comment%ROWTYPE;
    binding RECORD;
    operation TEXT;
BEGIN
    IF current_setting('patchbay.linear_remote_apply', true) = 'on' THEN
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
        JOIN issue i ON i.project_id=b.patchbay_project_id AND i.workspace_id=b.workspace_id
        WHERE i.id=source_comment.issue_id AND i.workspace_id=source_comment.workspace_id
          AND b.status='active' AND b.sync_mode IN ('publish','two_way')
    LOOP
        INSERT INTO linear_comment_link(workspace_id,binding_id,issue_id,comment_id,linear_comment_id,origin)
        VALUES(source_comment.workspace_id,binding.id,source_comment.issue_id,source_comment.id,gen_random_uuid()::text,'patchbay')
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

CREATE TRIGGER trg_linear_comment_outbox AFTER INSERT OR UPDATE OR DELETE ON comment
FOR EACH ROW EXECUTE FUNCTION enqueue_linear_comment_outbox();
