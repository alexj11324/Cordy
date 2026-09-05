package handler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	linearapi "github.com/patchbay-ai/patchbay/server/internal/integrations/linear"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

var linearSharedFields = []string{"title", "description", "status", "priority", "due_date", "owner_id"}

func linearSyncDate(value *string) pgtype.Date {
	if value == nil || strings.TrimSpace(*value) == "" { return pgtype.Date{} }
	parsed, err := time.Parse("2006-01-02", strings.TrimSpace(*value)); if err != nil { return pgtype.Date{} }
	return pgtype.Date{Time: parsed, Valid: true}
}

func (w *LinearWorker) accessToken(ctx context.Context, connectionID pgtype.UUID) (string, error) {
	tx, err := w.txStarter.Begin(ctx); if err != nil { return "", err }; defer tx.Rollback(ctx)
	var access, refresh []byte
	var expires pgtype.Timestamptz
	var status string
	if err = tx.QueryRow(ctx, `SELECT access_token_encrypted,refresh_token_encrypted,token_expires_at,status FROM linear_connection WHERE id=$1 FOR UPDATE`, connectionID).Scan(&access,&refresh,&expires,&status); err != nil { return "", err }
	if status != "active" { return "", fmt.Errorf("Linear connection status %s",status) }
	plain, err := w.box.Open(access); if err != nil { return "", err }
	if expires.Valid && time.Until(expires.Time) > 5*time.Minute { if err=tx.Commit(ctx);err!=nil{return "",err};return string(plain),nil }
	refreshPlain, err := w.box.Open(refresh); if err != nil { return "", err }
	if strings.TrimSpace(string(refreshPlain)) == "" { return "", errors.New("Linear refresh token is empty") }
	token, err := w.api.RefreshToken(ctx,string(refreshPlain),w.clientID,w.clientSecret)
	if err != nil {
		_, _ = tx.Exec(ctx,`UPDATE linear_connection SET status='reauthorization_required',last_error=$2,updated_at=now() WHERE id=$1`,connectionID,err.Error())
		_ = tx.Commit(ctx)
		return "", err
	}
	if strings.TrimSpace(token.AccessToken)=="" || strings.TrimSpace(token.RefreshToken)=="" || token.ExpiresIn<=0 { return "",errors.New("Linear refresh returned an invalid token") }
	newAccess, err := w.box.Seal([]byte(token.AccessToken)); if err != nil { return "",err }
	newRefresh, err := w.box.Seal([]byte(token.RefreshToken)); if err != nil { return "",err }
	if _,err=tx.Exec(ctx,`UPDATE linear_connection SET access_token_encrypted=$2,refresh_token_encrypted=$3,token_expires_at=$4,scopes=CASE WHEN $5='' THEN scopes ELSE to_jsonb(regexp_split_to_array($5,'[, ]+')) END,last_error=NULL,updated_at=now() WHERE id=$1 AND status='active'`,connectionID,newAccess,newRefresh,time.Now().UTC().Add(token.ExpiresIn),token.Scope);err!=nil{return "",err}
	if err=tx.Commit(ctx);err!=nil{return "",err};return token.AccessToken,nil
}

func linearSyncDateString(value pgtype.Date) any { if !value.Valid { return nil }; return value.Time.Format("2006-01-02") }

func linearSyncString(value any) string { if s, ok := value.(string); ok { return s }; return "" }

func linearSyncOwnerForLocal(ctx context.Context, tx pgx.Tx, b workerBinding, issue db.Issue) any {
	if !issue.OwnerType.Valid || issue.OwnerType.String != "member" || !issue.OwnerID.Valid { return nil }
	var linearUserID string
	if err := tx.QueryRow(ctx, `SELECT linear_user_id FROM linear_member_binding WHERE workspace_id=$1 AND connection_id=$2 AND patchbay_user_id=$3`, b.WorkspaceID, b.ConnectionID, issue.OwnerID).Scan(&linearUserID); err != nil { return nil }
	if strings.TrimSpace(linearUserID) == "" { return nil }
	return linearUserID
}

func linearSyncLocalSnapshot(ctx context.Context, tx pgx.Tx, b workerBinding, issue db.Issue) map[string]any {
	description := ""
	if issue.Description.Valid { description = linearapi.StripPatchbayIssueMarker(issue.Description.String) }
	return map[string]any{
		"title": issue.Title, "description": description, "status": issue.Status,
		"priority": issue.Priority, "due_date": linearSyncDateString(issue.DueDate),
		"owner_id": linearSyncOwnerForLocal(ctx, tx, b, issue),
	}
}

func linearSyncRemoteSnapshot(b workerBinding, issue linearapi.Issue) map[string]any {
	var due any
	if issue.DueDate != nil && strings.TrimSpace(*issue.DueDate) != "" { due = strings.TrimSpace(*issue.DueDate) }
	var owner any
	if strings.TrimSpace(issue.AssigneeID) != "" { owner = issue.AssigneeID }
	return map[string]any{
		"title": issue.Title, "description": linearapi.StripPatchbayIssueMarker(issue.Description),
		"status": remoteStatus(b, issue), "priority": remotePriority(issue.Priority),
		"due_date": due, "owner_id": owner,
	}
}

func linearSyncNormalizeBase(raw []byte, local map[string]any) map[string]any {
	base := map[string]any{}
	if len(raw) > 0 { _ = json.Unmarshal(raw, &base) }
	if priority, ok := base["priority"].(float64); ok { base["priority"] = remotePriority(int(priority)) }
	if state, ok := base["state"].(map[string]any); ok { if id, ok := state["id"].(string); ok { base["status"] = id } }
	if _, ok := base["owner_id"]; !ok { if assignee, ok := base["assignee_id"]; ok { base["owner_id"] = assignee } }
	for _, field := range linearSharedFields { if _, ok := base[field]; !ok { base[field] = local[field] } }
	return base
}

func linearSyncOwnerPatch(ctx context.Context, tx pgx.Tx, b workerBinding, value any, current db.Issue) (pgtype.Text, pgtype.UUID, error) {
	remoteID := linearSyncString(value)
	if remoteID == "" { return pgtype.Text{}, pgtype.UUID{}, nil }
	var patchbayUserID pgtype.UUID
	if err := tx.QueryRow(ctx, `SELECT patchbay_user_id FROM linear_member_binding WHERE workspace_id=$1 AND connection_id=$2 AND linear_user_id=$3`, b.WorkspaceID, b.ConnectionID, remoteID).Scan(&patchbayUserID); err != nil {
		// Unknown remote owners are preserved in the common snapshot. Clearing
		// the local owner here would turn an unmapped provider identity into a
		// destructive local mutation.
		return current.OwnerType, current.OwnerID, nil
	}
	return pgtype.Text{String: "member", Valid: true}, patchbayUserID, nil
}

func linearSyncUpdateParams(issue db.Issue, next map[string]any, projectID pgtype.UUID, ownerType pgtype.Text, ownerID pgtype.UUID) db.UpdateIssueParams {
	return db.UpdateIssueParams{
		ID: issue.ID, ExpectedRevision: pgtype.Int8{Int64: issue.Revision, Valid: true},
		Title: pgtype.Text{String: linearSyncString(next["title"]), Valid: true},
		Description: pgtype.Text{String: linearSyncString(next["description"]), Valid: true},
		Status: pgtype.Text{String: linearSyncString(next["status"]), Valid: true},
		Priority: pgtype.Text{String: linearSyncString(next["priority"]), Valid: true},
		ExecutorType: issue.ExecutorType, ExecutorID: issue.ExecutorID,
		OwnerType: ownerType, OwnerID: ownerID,
		ReviewerType: issue.ReviewerType, ReviewerID: issue.ReviewerID,
		Position: pgtype.Float8{Float64: issue.Position, Valid: true},
		StartDate: issue.StartDate, DueDate: linearSyncDate(linearSyncStringPointer(next["due_date"])),
		ParentIssueID: issue.ParentIssueID, ProjectID: projectID, Stage: issue.Stage,
	}
}

func linearSyncStringPointer(value any) *string { if s, ok := value.(string); ok && strings.TrimSpace(s) != "" { return &s }; return nil }

func (w *LinearWorker) applyRemote(ctx context.Context, b workerBinding, remote linearapi.Issue, eventID string, eventAt int64) error {
	if strings.TrimSpace(remote.ID) == "" { return errors.New("Linear issue omitted id") }
	if eventAt <= 0 && !remote.UpdatedAt.IsZero() { eventAt = remote.UpdatedAt.UnixMilli() }
	if eventID == "" { eventID = "remote:" + remote.ID + ":" + fmt.Sprint(eventAt) }
	tx, err := w.txStarter.Begin(ctx); if err != nil { return err }; defer tx.Rollback(ctx)
	if _, err = tx.Exec(ctx, `SELECT set_config('patchbay.linear_remote_apply','on',true)`); err != nil { return err }
	queries := db.New(tx)
	link, linkErr := queries.GetLinearIssueLinkByRemote(ctx, db.GetLinearIssueLinkByRemoteParams{BindingID:b.ID,LinearIssueID:remote.ID})
	if errors.Is(linkErr, pgx.ErrNoRows) {
		// A remote project move can change the matching binding. Locate the
		// existing link through the connection and rebind it atomically.
		linkErr = tx.QueryRow(ctx, `SELECT l.id,l.workspace_id,l.binding_id,l.patchbay_issue_id,l.linear_issue_id,l.linear_identifier,l.last_common_snapshot,l.remote_updated_at,l.last_remote_event_at_ms,l.last_remote_event_id,l.sync_status,l.created_at,l.updated_at FROM linear_issue_link l JOIN linear_project_binding old_b ON old_b.id=l.binding_id WHERE old_b.connection_id=$1 AND l.linear_issue_id=$2 AND l.sync_status<>'deleted' ORDER BY l.updated_at DESC LIMIT 1 FOR UPDATE`, b.ConnectionID, remote.ID).Scan(&link.ID,&link.WorkspaceID,&link.BindingID,&link.PatchbayIssueID,&link.LinearIssueID,&link.LinearIdentifier,&link.LastCommonSnapshot,&link.RemoteUpdatedAt,&link.LastRemoteEventAtMs,&link.LastRemoteEventID,&link.SyncStatus,&link.CreatedAt,&link.UpdatedAt)
	}
	if errors.Is(linkErr, pgx.ErrNoRows) {
		if remote.Deleted { return tx.Commit(ctx) }
		originID, parseErr := uuid.Parse(remote.ID); if parseErr != nil { return fmt.Errorf("Linear issue id is not UUID: %w", parseErr) }
		issue, originErr := queries.GetIssueByOrigin(ctx, db.GetIssueByOriginParams{WorkspaceID:b.WorkspaceID,OriginType:pgtype.Text{String:"linear",Valid:true},OriginID:pgtype.UUID{Bytes:originID,Valid:true}})
		if errors.Is(originErr, pgx.ErrNoRows) {
			var number int32; if err = tx.QueryRow(ctx, `UPDATE workspace SET issue_counter=issue_counter+1 WHERE id=$1 RETURNING issue_counter`, b.WorkspaceID).Scan(&number); err != nil { return err }
			var position float64; _ = tx.QueryRow(ctx, `SELECT COALESCE(MIN(position)-1,0) FROM issue WHERE workspace_id=$1 AND status=$2`, b.WorkspaceID, remoteStatus(b,remote)).Scan(&position)
			ownerType, ownerID, ownerErr := linearSyncOwnerPatch(ctx,tx,b,remote.AssigneeID,db.Issue{}); if ownerErr != nil { return ownerErr }
			issue, err = queries.CreateIssueWithOrigin(ctx, db.CreateIssueWithOriginParams{WorkspaceID:b.WorkspaceID,Title:remote.Title,Description:pgtype.Text{String:linearapi.StripPatchbayIssueMarker(remote.Description),Valid:true},Status:remoteStatus(b,remote),Priority:remotePriority(remote.Priority),CreatorType:"member",CreatorID:b.CreatorID,Position:position,DueDate:linearSyncDate(remote.DueDate),Number:number,ProjectID:b.ProjectID,OriginType:pgtype.Text{String:"linear",Valid:true},OriginID:pgtype.UUID{Bytes:originID,Valid:true},ID:dbid.NewV7(),OwnerType:ownerType,OwnerID:ownerID})
			if err != nil { return err }
		} else if originErr != nil { return originErr }
		base, _ := json.Marshal(linearSyncRemoteSnapshot(b,remote))
		link, err = queries.CreateLinearIssueLink(ctx, db.CreateLinearIssueLinkParams{ID:parseUUID(uuid.NewString()),WorkspaceID:b.WorkspaceID,BindingID:b.ID,PatchbayIssueID:issue.ID,LinearIssueID:remote.ID,LinearIdentifier:remote.Identifier,LastCommonSnapshot:base,RemoteUpdatedAt:pgtype.Timestamptz{Time:remote.UpdatedAt,Valid:!remote.UpdatedAt.IsZero()},LastRemoteEventAtMs:pgtype.Int8{Int64:eventAt,Valid:eventAt>0},LastRemoteEventID:pgtype.Text{String:eventID,Valid:true}})
		if err != nil { return err }
		if err = tx.Commit(ctx); err != nil { return err }
		w.publishIssueEvent(issue,"issue:created")
		return nil
	}
	if linkErr != nil { return linkErr }
	if link.BindingID != b.ID { if _, err = tx.Exec(ctx, `UPDATE linear_issue_link SET binding_id=$2,updated_at=now() WHERE id=$1 AND workspace_id=$3`,link.ID,b.ID,b.WorkspaceID); err != nil { return err }; link.BindingID=b.ID }
	if link.LastRemoteEventID.Valid && link.LastRemoteEventID.String == eventID { return tx.Commit(ctx) }
	if eventAt > 0 && link.LastRemoteEventAtMs.Valid && eventAt <= link.LastRemoteEventAtMs.Int64 { return tx.Commit(ctx) }
	issue, err := queries.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID:link.PatchbayIssueID,WorkspaceID:b.WorkspaceID}); if err != nil { return err }
	if remote.Deleted {
		updated, updateErr := queries.UpdateIssue(ctx,linearSyncUpdateParams(issue,map[string]any{"title":issue.Title,"description":issue.Description.String,"status":"cancelled","priority":issue.Priority,"due_date":linearSyncDateString(issue.DueDate)},issue.ProjectID,issue.OwnerType,issue.OwnerID)); if updateErr != nil { return updateErr }
		if err = queries.UpdateLinearIssueLink(ctx,db.UpdateLinearIssueLinkParams{ID:link.ID,WorkspaceID:b.WorkspaceID,LinearIdentifier:link.LinearIdentifier,LastCommonSnapshot:link.LastCommonSnapshot,RemoteUpdatedAt:pgtype.Timestamptz{Time:remote.UpdatedAt,Valid:!remote.UpdatedAt.IsZero()},LastRemoteEventAtMs:pgtype.Int8{Int64:eventAt,Valid:eventAt>0},LastRemoteEventID:pgtype.Text{String:eventID,Valid:true},SyncStatus:"deleted"}); err != nil { return err }
		if err = tx.Commit(ctx); err != nil { return err }; w.publishIssueEvent(updated,"issue:updated"); return nil
	}
	local := linearSyncLocalSnapshot(ctx,tx,b,issue)
	base := linearSyncNormalizeBase(link.LastCommonSnapshot,local)
	incoming := linearSyncRemoteSnapshot(b,remote)
	next := map[string]any{}
	conflicted := false
	changed := false
	for _, field := range linearSharedFields {
		localChanged := !valueEqual(local[field],base[field]); remoteChanged := !valueEqual(incoming[field],base[field])
		switch {
		case localChanged && remoteChanged && !valueEqual(local[field],incoming[field]):
			rawBase,_:=json.Marshal(base[field]); rawLocal,_:=json.Marshal(local[field]); rawRemote,_:=json.Marshal(incoming[field])
			if err = queries.CreateLinearSyncConflict(ctx,db.CreateLinearSyncConflictParams{ID:parseUUID(uuid.NewString()),WorkspaceID:b.WorkspaceID,BindingID:b.ID,LinkID:link.ID,PatchbayIssueID:issue.ID,LinearIssueID:remote.ID,Field:field,BaseValue:rawBase,LocalValue:rawLocal,RemoteValue:rawRemote,SourceEventID:eventID,SourceEventAtMs:pgtype.Int8{Int64:eventAt,Valid:eventAt>0}}); err != nil { return err }
			next[field] = local[field]; conflicted = true
		case remoteChanged:
			next[field] = incoming[field]
		default:
			next[field] = local[field]
		}
		if !valueEqual(next[field],local[field]) { changed = true }
	}
	ownerType, ownerID, ownerErr := linearSyncOwnerPatch(ctx,tx,b,next["owner_id"],issue); if ownerErr != nil { return ownerErr }
	if changed || issue.ProjectID != b.ProjectID {
		issue, err = queries.UpdateIssue(ctx,linearSyncUpdateParams(issue,next,b.ProjectID,ownerType,ownerID)); if err != nil { return err }
		details,_:=json.Marshal(map[string]any{"source":"linear","event_id":eventID,"remote_id":remote.ID}); if _, err = queries.CreateActivity(ctx,db.CreateActivityParams{ID:dbid.NewV7(),WorkspaceID:b.WorkspaceID,IssueID:issue.ID,ActorType:pgtype.Text{String:"system",Valid:true},Action:"linear_sync_applied",Details:details}); err != nil { return err }
	}
	common := incoming; if conflicted { for _, field := range linearSharedFields { if valueEqual(next[field],local[field]) && !valueEqual(local[field],incoming[field]) { common[field] = local[field] } } }
	snapshot, _ := json.Marshal(common)
	status := "active"; if conflicted { status = "conflict" }
	if err = queries.UpdateLinearIssueLink(ctx,db.UpdateLinearIssueLinkParams{ID:link.ID,WorkspaceID:b.WorkspaceID,LinearIdentifier:remote.Identifier,LastCommonSnapshot:snapshot,RemoteUpdatedAt:pgtype.Timestamptz{Time:remote.UpdatedAt,Valid:!remote.UpdatedAt.IsZero()},LastRemoteEventAtMs:pgtype.Int8{Int64:eventAt,Valid:eventAt>0},LastRemoteEventID:pgtype.Text{String:eventID,Valid:true},SyncStatus:status}); err != nil { return err }
	if err = tx.Commit(ctx); err != nil { return err }
	if changed { w.publishIssueEvent(issue,"issue:updated") }
	return nil
}

func linearSyncLocalIssueInput(ctx context.Context, tx pgx.Tx, b workerBinding, issue db.Issue) (linearapi.IssueInput, error) {
	input := linearapi.IssueInput{TeamID:b.TeamID.String,ProjectID:b.LinearProjectID,Title:issue.Title,Description:linearapi.StripPatchbayIssueMarker(issue.Description.String),Priority:localPriority(issue.Priority),StateID:stateForLocal(b,issue.Status),PatchbayIssueID:uuidToString(issue.ID)}
	if issue.DueDate.Valid { due := issue.DueDate.Time.Format("2006-01-02"); input.DueDate = &due }
	if issue.OwnerID.Valid && issue.OwnerType.Valid && issue.OwnerType.String == "member" {
		var linearUserID string
		if err := tx.QueryRow(ctx, `SELECT linear_user_id FROM linear_member_binding WHERE workspace_id=$1 AND connection_id=$2 AND patchbay_user_id=$3`, b.WorkspaceID,b.ConnectionID,issue.OwnerID).Scan(&linearUserID); err != nil { return input, errors.New("Linear owner is not mapped to a Linear user") }
		input.AssigneeID = &linearUserID
	} else if issue.OwnerID.Valid || issue.OwnerType.Valid { return input, errors.New("Linear publish only supports mapped human owners") } else { input.ClearAssignee = true }
	return input,nil
}

func (w *LinearWorker) completeOutboxInTx(ctx context.Context, tx pgx.Tx, id pgtype.UUID) error {
	var processed bool
	if err := tx.QueryRow(ctx, `UPDATE linear_sync_outbox SET processed_at=now(),locked_by=NULL,locked_until=NULL,last_error=NULL,updated_at=now() WHERE id=$1 AND locked_by=$2 RETURNING true`, id,w.workerID).Scan(&processed); err != nil { return err }
	return nil
}

func (w *LinearWorker) handleOutbox(ctx context.Context, c linearOutboxClaim) error {
	b, err := w.loadBinding(ctx,c.BindingID)
	if errors.Is(err,pgx.ErrNoRows) { return nil }
	if err != nil { return err }
	if b.WorkspaceID != c.WorkspaceID { return errors.New("Linear outbox workspace mismatch") }
	if b.Mode != "publish" && b.Mode != "two_way" { return nil }
	token, err := w.accessToken(ctx,b.ConnectionID); if err != nil { return err }
	if strings.HasPrefix(c.EventType, "comment_") { return w.handleCommentOutbox(ctx,c,b,token) }
	if c.EventType == "attachment_deleted" { return w.deleteLinearWorkProductAttachment(ctx,b,c.IssueID,token,c.Payload) }
	queries := db.New(w.db)
	issue, err := queries.GetIssueInWorkspace(ctx,db.GetIssueInWorkspaceParams{ID:c.IssueID,WorkspaceID:b.WorkspaceID})
	if err != nil && c.EventType != "issue_deleted" { return err }
	link, linkErr := queries.GetLinearIssueLinkByLocal(ctx,db.GetLinearIssueLinkByLocalParams{WorkspaceID:b.WorkspaceID,BindingID:b.ID,PatchbayIssueID:c.IssueID})
	if c.EventType == "issue_deleted" {
		if errors.Is(linkErr,pgx.ErrNoRows) { return nil }
		if linkErr != nil { return linkErr }
		if err = w.api.DeleteIssue(ctx,token,link.LinearIssueID); err != nil { return err }
		tx, txErr := w.txStarter.Begin(ctx); if txErr != nil { return txErr }; defer tx.Rollback(ctx)
		if _, txErr = tx.Exec(ctx,`UPDATE linear_issue_link SET sync_status='deleted',updated_at=now() WHERE id=$1 AND workspace_id=$2`,link.ID,b.WorkspaceID); txErr != nil { return txErr }
		if txErr = w.completeOutboxInTx(ctx,tx,c.ID); txErr != nil { return txErr }
		return tx.Commit(ctx)
	}
	if err != nil { return err }
	if linkErr != nil && !errors.Is(linkErr,pgx.ErrNoRows) { return linkErr }
	if linkErr == nil && link.SyncStatus == "conflict" { return errors.New("Linear issue link has an open conflict") }
	if linkErr == nil && link.RemoteUpdatedAt.Valid {
		remote, found, fetchErr := w.api.FetchIssue(ctx,token,link.LinearIssueID); if fetchErr != nil { return fetchErr }
		if found && remote.UpdatedAt.After(link.RemoteUpdatedAt.Time) {
			if err = w.applyRemote(ctx,b,remote,"prepush:"+c.EventType+":"+uuidToString(c.ID),remote.UpdatedAt.UnixMilli()); err != nil { return err }
			link, linkErr = queries.GetLinearIssueLinkByLocal(ctx,db.GetLinearIssueLinkByLocalParams{WorkspaceID:b.WorkspaceID,BindingID:b.ID,PatchbayIssueID:c.IssueID}); if linkErr != nil { return linkErr }
			if link.SyncStatus == "conflict" { return errors.New("Linear issue link has an open conflict") }
			issue, err = queries.GetIssueInWorkspace(ctx,db.GetIssueInWorkspaceParams{ID:c.IssueID,WorkspaceID:b.WorkspaceID}); if err != nil { return err }
		}
	}
	readTx, err := w.txStarter.Begin(ctx); if err != nil { return err }
	input, err := linearSyncLocalIssueInput(ctx,readTx,b,issue); _ = readTx.Rollback(ctx); if err != nil { return err }
	var remote linearapi.Issue
	if errors.Is(linkErr,pgx.ErrNoRows) {
		// If a previous provider create succeeded before the local transaction
		// was lost, recover it by the stable marker instead of creating a
		// duplicate Linear issue.
		if c.Attempts > 1 {
			listed, listErr := w.api.ListIssues(ctx,token,b.LinearProjectID,b.TeamID.String); if listErr != nil { return listErr }
			for _, candidate := range listed { if linearapi.PatchbayIssueIDFromDescription(candidate.Description) == uuidToString(c.IssueID) { remote = candidate; break } }
		}
		if remote.ID == "" { remote, err = w.api.CreateIssue(ctx,token,input) } else { remote, err = w.api.UpdateIssue(ctx,token,remote.ID,input) }
	} else { remote, err = w.api.UpdateIssue(ctx,token,link.LinearIssueID,input) }
	if err != nil { return err }
	if strings.TrimSpace(remote.ID) == "" || strings.TrimSpace(remote.Identifier) == "" { return errors.New("Linear mutation returned an incomplete issue") }
	if remote.UpdatedAt.IsZero() { remote.UpdatedAt = time.Now().UTC() }
	if err = w.publishLinearWorkProducts(ctx,b,c.IssueID,remote.ID,token);err!=nil{return err}
	tx, err := w.txStarter.Begin(ctx); if err != nil { return err }; defer tx.Rollback(ctx)
	if _, err = tx.Exec(ctx,`SELECT set_config('patchbay.linear_remote_apply','on',true)`); err != nil { return err }
	qtx := db.New(tx)
	snapshot, _ := json.Marshal(linearSyncLocalSnapshot(ctx,tx,b,issue))
	remoteTime := pgtype.Timestamptz{Time:remote.UpdatedAt,Valid:true}
	if errors.Is(linkErr,pgx.ErrNoRows) {
		link, err = qtx.CreateLinearIssueLink(ctx,db.CreateLinearIssueLinkParams{ID:parseUUID(uuid.NewString()),WorkspaceID:b.WorkspaceID,BindingID:b.ID,PatchbayIssueID:c.IssueID,LinearIssueID:remote.ID,LinearIdentifier:remote.Identifier,LastCommonSnapshot:snapshot,RemoteUpdatedAt:remoteTime,LastRemoteEventAtMs:pgtype.Int8{Int64:remote.UpdatedAt.UnixMilli(),Valid:true},LastRemoteEventID:pgtype.Text{String:"local:"+uuidToString(c.ID),Valid:true}}); if err != nil { return err }
	} else {
		if err = qtx.UpdateLinearIssueLink(ctx,db.UpdateLinearIssueLinkParams{ID:link.ID,WorkspaceID:b.WorkspaceID,LinearIdentifier:remote.Identifier,LastCommonSnapshot:snapshot,RemoteUpdatedAt:remoteTime,LastRemoteEventAtMs:pgtype.Int8{Int64:remote.UpdatedAt.UnixMilli(),Valid:true},LastRemoteEventID:pgtype.Text{String:"local:"+uuidToString(c.ID),Valid:true},SyncStatus:"active"}); err != nil { return err }
	}
	if err = w.completeOutboxInTx(ctx,tx,c.ID); err != nil { return err }
	return tx.Commit(ctx)
}
