package handler

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

func linearConflictValue(raw []byte) (any, error) {
	if len(raw) == 0 { return nil, nil }
	var value any
	if err := json.Unmarshal(raw, &value); err != nil { return nil, err }
	return value, nil
}

func linearConflictField(field string) bool { for _, candidate := range linearSharedFields { if candidate == field { return true } }; return false }

func (h *Handler) resolveLinearConflict(ctx context.Context, workspaceID, conflictID, actorID pgtype.UUID, resolution string, manual any) (db.LinearSyncConflict, db.Issue, error) {
	if resolution != "local" && resolution != "remote" && resolution != "manual" { return db.LinearSyncConflict{}, db.Issue{}, errors.New("invalid Linear conflict resolution") }
	tx, err := h.TxStarter.Begin(ctx); if err != nil { return db.LinearSyncConflict{}, db.Issue{}, err }; defer tx.Rollback(ctx)
	if _, err = tx.Exec(ctx, `SELECT set_config('patchbay.linear_remote_apply','on',true)`); err != nil { return db.LinearSyncConflict{}, db.Issue{}, err }
	qtx := db.New(tx)
	conflict, err := qtx.GetLinearSyncConflictForUpdate(ctx,db.GetLinearSyncConflictForUpdateParams{ID:conflictID,WorkspaceID:workspaceID}); if errors.Is(err,pgx.ErrNoRows) { return db.LinearSyncConflict{},db.Issue{},pgx.ErrNoRows }; if err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	if conflict.Status != "open" { return db.LinearSyncConflict{},db.Issue{},errors.New("Linear conflict is already resolved") }
	if !linearConflictField(conflict.Field) { return db.LinearSyncConflict{},db.Issue{},errors.New("Linear conflict field is not syncable") }
	var selected any
	switch resolution { case "local": selected, err = linearConflictValue(conflict.LocalValue); case "remote": selected, err = linearConflictValue(conflict.RemoteValue); case "manual": selected = manual }
	if err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	if conflict.Field == "due_date" { if value, ok := selected.(string); ok { if _, parseErr := parseLinearDate(value); parseErr != nil { return db.LinearSyncConflict{},db.Issue{},errors.New("manual due_date must be YYYY-MM-DD") } } else if selected != nil { return db.LinearSyncConflict{},db.Issue{},errors.New("manual due_date must be a date or null") } }
	if conflict.Field == "owner_id" { if value, ok := selected.(string); ok && strings.TrimSpace(value) == "" { selected = nil } else if selected != nil && !ok { return db.LinearSyncConflict{},db.Issue{},errors.New("manual owner_id must be a Linear user id or null") } }
	binding, err := qtx.GetLinearProjectBindingForUpdate(ctx,db.GetLinearProjectBindingForUpdateParams{ID:conflict.BindingID,WorkspaceID:workspaceID}); if err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	if binding.Status != "active" || (binding.SyncMode != "two_way" && binding.SyncMode != "publish" && binding.SyncMode != "import") { return db.LinearSyncConflict{},db.Issue{},errors.New("Linear binding is not active") }
	var connectionStatus string; if err = tx.QueryRow(ctx,`SELECT status FROM linear_connection WHERE id=$1 AND workspace_id=$2`,binding.ConnectionID,workspaceID).Scan(&connectionStatus); err != nil || connectionStatus != "active" { return db.LinearSyncConflict{},db.Issue{},errors.New("Linear connection is not active") }
	link, err := qtx.GetLinearIssueLinkByRemote(ctx,db.GetLinearIssueLinkByRemoteParams{BindingID:binding.ID,LinearIssueID:conflict.LinearIssueID}); if err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	issue, err := qtx.GetIssueInWorkspace(ctx,db.GetIssueInWorkspaceParams{ID:conflict.PatchbayIssueID,WorkspaceID:workspaceID}); if err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	next := linearSyncLocalSnapshot(ctx,tx,workerBinding{WorkspaceID:workspaceID,ConnectionID:binding.ConnectionID},issue)
	next[conflict.Field] = selected
	ownerType, ownerID := issue.OwnerType, issue.OwnerID
	if conflict.Field == "owner_id" { ownerType, ownerID, err = linearSyncOwnerPatch(ctx,tx,workerBinding{WorkspaceID:workspaceID,ConnectionID:binding.ConnectionID},selected,issue); if err != nil { return db.LinearSyncConflict{},db.Issue{},err }; if linearSyncString(selected) != "" && !ownerID.Valid { return db.LinearSyncConflict{},db.Issue{},errors.New("selected Linear owner is not mapped") } }
	if conflict.Field == "owner_id" && selected == nil { ownerType, ownerID = pgtype.Text{},pgtype.UUID{} }
	updated, err := qtx.UpdateIssue(ctx,linearSyncUpdateParams(issue,next,issue.ProjectID,ownerType,ownerID)); if err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	resolvedValue,_:=json.Marshal(selected)
	if _, err = qtx.ResolveLinearSyncConflict(ctx,db.ResolveLinearSyncConflictParams{ID:conflict.ID,WorkspaceID:workspaceID,Resolution:pgtype.Text{String:resolution,Valid:true},ResolvedValue:resolvedValue,ResolvedByID:actorID}); err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	var common map[string]any; _ = json.Unmarshal(link.LastCommonSnapshot,&common); if common == nil { common=map[string]any{} }; common[conflict.Field]=selected; commonRaw,_:=json.Marshal(common)
	openCount := 0; if err = tx.QueryRow(ctx,`SELECT count(*) FROM linear_sync_conflict WHERE link_id=$1 AND status='open' AND id<>$2`,link.ID,conflict.ID).Scan(&openCount); err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	linkStatus := "active"; if openCount > 0 { linkStatus = "conflict" }
	if err = qtx.UpdateLinearIssueLink(ctx,db.UpdateLinearIssueLinkParams{ID:link.ID,WorkspaceID:workspaceID,LinearIdentifier:link.LinearIdentifier,LastCommonSnapshot:commonRaw,RemoteUpdatedAt:link.RemoteUpdatedAt,LastRemoteEventAtMs:link.LastRemoteEventAtMs,LastRemoteEventID:link.LastRemoteEventID,SyncStatus:linkStatus}); err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	remoteValue,_:=linearConflictValue(conflict.RemoteValue)
	if (resolution == "local" || resolution == "manual") && !valueEqual(selected,remoteValue) && (binding.SyncMode == "publish" || binding.SyncMode == "two_way") { payload,_:=json.Marshal(map[string]any{"id":issue.ID,"revision":updated.Revision,"source":"linear_conflict"}); if _,err=tx.Exec(ctx,`INSERT INTO linear_sync_outbox(id,workspace_id,binding_id,issue_id,event_key,event_type,payload) VALUES(gen_random_uuid(),$1,$2,$3,$4,'issue_updated',$5) ON CONFLICT(binding_id,event_key) DO NOTHING`,workspaceID,binding.ID,issue.ID,"conflict-reconcile:"+uuidToString(conflict.ID),payload); err != nil { return db.LinearSyncConflict{},db.Issue{},err } }
	details,_:=json.Marshal(map[string]any{"source":"linear","conflict_id":conflict.ID,"resolution":resolution}); if _,err=qtx.CreateActivity(ctx,db.CreateActivityParams{ID:dbid.NewV7(),WorkspaceID:workspaceID,IssueID:issue.ID,ActorType:pgtype.Text{String:"member",Valid:true},ActorID:actorID,Action:"linear_conflict_resolved",Details:details}); err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	if err = tx.Commit(ctx); err != nil { return db.LinearSyncConflict{},db.Issue{},err }
	if h.Bus != nil { h.Bus.Publish(events.Event{Type:"issue:updated",WorkspaceID:uuidToString(workspaceID),ActorType:"member",ActorID:uuidToString(actorID),Payload:map[string]any{"issue":service.IssueToMap(updated,"")},TaskID:uuidToString(updated.ID)}) }
	conflict.Status, conflict.Resolution, conflict.ResolvedValue, conflict.ResolvedByID = "resolved",pgtype.Text{String:resolution,Valid:true},resolvedValue,actorID
	return conflict,updated,nil
}

func parseLinearDate(value string) (string,error) { if len(value)!=10 { return "",errors.New("invalid date") }; if _,err:=time.Parse("2006-01-02",value);err!=nil{return "",err};return value,nil }
