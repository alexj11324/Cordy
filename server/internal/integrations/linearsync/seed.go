package linearsync

import (
	"context"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
)

// SeedOutbound adds existing issues and discussion to a new binding within
// the caller's binding transaction. Issue and parent-comment ordering is
// preserved by the existing event keys and creation timestamps.
func SeedOutbound(ctx context.Context, tx pgx.Tx, ws, bindingID, projectID pgtype.UUID) error {
	if _, err := tx.Exec(ctx, `INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload) SELECT gen_random_uuid(),i.workspace_id,$2::uuid,i.id,'binding-seed:'||$2::uuid::text||':'||i.id::text||':'||i.revision::text,'issue_updated',jsonb_build_object('id',i.id,'revision',i.revision) FROM issue i WHERE i.workspace_id=$1::uuid AND i.project_id=$3::uuid AND i.status<>'cancelled' ON CONFLICT(binding_id,event_key) DO NOTHING`, ws, bindingID, projectID); err != nil {
		return err
	}
	if _, err := tx.Exec(ctx, `INSERT INTO linear_comment_link(workspace_id,binding_id,issue_id,comment_id,linear_comment_id,origin) SELECT c.workspace_id,$2::uuid,c.issue_id,c.id,gen_random_uuid()::text,'orvilo' FROM comment c JOIN issue i ON i.id=c.issue_id AND i.workspace_id=c.workspace_id WHERE c.workspace_id=$1::uuid AND i.project_id=$3::uuid AND i.status<>'cancelled' AND c.author_type IN ('member','agent') AND c.type='comment' ON CONFLICT(binding_id,comment_id) DO NOTHING`, ws, bindingID, projectID); err != nil {
		return err
	}
	_, err := tx.Exec(ctx, `WITH RECURSIVE candidates AS (SELECT c.* FROM comment c JOIN issue i ON i.id=c.issue_id AND i.workspace_id=c.workspace_id WHERE c.workspace_id=$1::uuid AND i.project_id=$3::uuid AND i.status<>'cancelled' AND c.author_type IN ('member','agent') AND c.type='comment'), ordered AS (SELECT c.*,0 AS depth FROM candidates c WHERE c.parent_id IS NULL OR NOT EXISTS (SELECT 1 FROM candidates parent WHERE parent.id=c.parent_id) UNION ALL SELECT child.*,parent.depth+1 FROM candidates child JOIN ordered parent ON child.parent_id=parent.id) INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload,created_at) SELECT gen_random_uuid(),c.workspace_id,$2::uuid,c.issue_id,'binding-seed-comment:'||$2::uuid::text||':'||c.id::text||':'||c.revision::text,'comment_created',jsonb_build_object('comment_id',c.id,'body',c.content,'parent_id',CASE WHEN EXISTS (SELECT 1 FROM candidates parent WHERE parent.id=c.parent_id) THEN c.parent_id ELSE NULL END,'author_type',c.author_type,'author_id',c.author_id),transaction_timestamp()+((c.depth+1)*interval '1 millisecond') FROM ordered c ON CONFLICT(binding_id,event_key) DO NOTHING`, ws, bindingID, projectID)
	return err
}
