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
	"github.com/patchbay-ai/patchbay/server/internal/events"
	linearapi "github.com/patchbay-ai/patchbay/server/internal/integrations/linear"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

type linearCommentAPI interface {
	FetchComment(context.Context, string, string) (linearapi.Comment, bool, error)
	ListComments(context.Context, string, string) ([]linearapi.Comment, error)
	CreateComment(context.Context, string, string, string, string, string, string) (linearapi.Comment, error)
	UpdateComment(context.Context, string, string, string) error
	DeleteComment(context.Context, string, string) error
}

func (w *LinearWorker) importComments(ctx context.Context, b workerBinding, token, issueID string) error {
	api, ok := w.api.(linearCommentAPI)
	if !ok {
		return errors.New("Linear comment API is unavailable")
	}
	comments, err := api.ListComments(ctx, token, issueID)
	if err != nil {
		return err
	}
	pending := make(map[string]linearapi.Comment, len(comments))
	for _, c := range comments {
		pending[c.ID] = c
	}
	for len(pending) > 0 {
		progress := false
		for id, c := range pending {
			if c.Parent != nil {
				if _, waiting := pending[c.Parent.ID]; waiting {
					continue
				}
			}
			if err := w.applyLinearComment(ctx, b, c, false); err != nil {
				return err
			}
			delete(pending, id)
			progress = true
		}
		if !progress {
			return errors.New("Linear comment reply graph contains a cycle")
		}
	}
	return nil
}

func (w *LinearWorker) handleCommentInbox(ctx context.Context, claim linearClaim) error {
	var envelope struct {
		Action string `json:"action"`
		Data   struct {
			ID      string `json:"id"`
			IssueID string `json:"issueId"`
		} `json:"data"`
		WebhookTimestamp int64 `json:"webhookTimestamp"`
	}
	if err := json.Unmarshal(claim.Payload, &envelope); err != nil {
		return err
	}
	if envelope.Data.ID == "" {
		return errors.New("Linear comment event omitted identity")
	}
	// Linear also emits Comment events for projects and documents.
	if envelope.Data.IssueID == "" {
		return nil
	}
	var bindingID pgtype.UUID
	err := w.db.QueryRow(ctx, `SELECT b.id FROM linear_project_binding b JOIN linear_issue_link l ON l.binding_id=b.id AND l.workspace_id=b.workspace_id WHERE b.connection_id=$1 AND l.linear_issue_id=$2 AND b.status='active' AND b.sync_mode IN ('import','two_way') AND l.sync_status<>'deleted'`, claim.ConnectionID, envelope.Data.IssueID).Scan(&bindingID)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return err
	}
	b, err := w.loadBinding(ctx, bindingID)
	if err != nil {
		return err
	}
	api, ok := w.api.(linearCommentAPI)
	if !ok {
		return errors.New("Linear comment API is unavailable")
	}
	token, err := w.accessToken(ctx, b.ConnectionID)
	if err != nil {
		return err
	}
	comment, found, err := api.FetchComment(ctx, token, envelope.Data.ID)
	if err != nil {
		return err
	}
	if !found {
		if envelope.Action != "remove" {
			return errors.New("Linear comment disappeared before it could be read")
		}
		comment.ID, comment.Issue.ID = envelope.Data.ID, envelope.Data.IssueID
		comment.UpdatedAt = time.UnixMilli(envelope.WebhookTimestamp)
	}
	if comment.Issue.ID != envelope.Data.IssueID {
		return errors.New("Linear comment issue mismatch")
	}
	return w.applyLinearComment(ctx, b, comment, !found)
}

// Imported discussion is stored as system-authored text with explicit source
// attribution. It does not impersonate local members or trigger agent runs.
func (w *LinearWorker) applyLinearComment(ctx context.Context, b workerBinding, remote linearapi.Comment, deleted bool) error {
	tx, err := w.txStarter.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx)
	var issueID pgtype.UUID
	if err = tx.QueryRow(ctx, `SELECT i.id FROM issue i JOIN linear_issue_link l ON l.patchbay_issue_id=i.id AND l.workspace_id=i.workspace_id JOIN linear_project_binding b ON b.id=l.binding_id WHERE l.binding_id=$1 AND l.linear_issue_id=$2 AND l.sync_status<>'deleted' AND b.status='active' AND b.sync_mode IN ('import','two_way') AND i.workspace_id=$3 FOR UPDATE OF i`, b.ID, remote.Issue.ID, b.WorkspaceID).Scan(&issueID); err != nil {
		return err
	}
	var localID pgtype.UUID
	var origin string
	var remoteTime pgtype.Timestamptz
	var tombstone bool
	err = tx.QueryRow(ctx, `SELECT comment_id,origin,remote_updated_at,deleted FROM linear_comment_link WHERE binding_id=$1 AND linear_comment_id=$2 AND workspace_id=$3 FOR UPDATE`, b.ID, remote.ID, b.WorkspaceID).Scan(&localID, &origin, &remoteTime, &tombstone)
	newLink := errors.Is(err, pgx.ErrNoRows)
	if err != nil && !newLink {
		return err
	}
	// The originating system owns its comment edits. This also suppresses the
	// webhook for our own commentCreate before its local acknowledgement.
	if origin == "patchbay" || tombstone || (remoteTime.Valid && !remote.UpdatedAt.After(remoteTime.Time)) {
		return tx.Commit(ctx)
	}
	if deleted && newLink {
		return tx.Commit(ctx)
	}
	if _, err = tx.Exec(ctx, `SELECT set_config('patchbay.linear_remote_apply','on',true)`); err != nil {
		return err
	}
	parentID := pgtype.UUID{}
	if remote.Parent != nil && !deleted {
		if err = tx.QueryRow(ctx, `SELECT comment_id FROM linear_comment_link WHERE binding_id=$1 AND linear_comment_id=$2 AND issue_id=$3 AND workspace_id=$4`, b.ID, remote.Parent.ID, issueID, b.WorkspaceID).Scan(&parentID); err != nil {
			return fmt.Errorf("Linear reply parent is not imported: %w", err)
		}
	}
	author := "Linear user"
	if remote.User != nil && strings.TrimSpace(remote.User.Name) != "" {
		author = strings.NewReplacer("\n", " ", "\r", " ", "[", "", "]", "").Replace(remote.User.Name)
	}
	body := author + " · Linear\n\n" + sanitizeNullBytes(remote.Body)
	if deleted {
		body = "[Comment deleted in Linear]"
	}
	q := db.New(tx)
	var comment db.Comment
	var issueRevision int64
	if newLink {
		localID = dbid.NewV7()
		created, createErr := q.CreateComment(ctx, db.CreateCommentParams{ID: localID, IssueID: issueID, WorkspaceID: b.WorkspaceID, AuthorType: "system", AuthorID: parseUUID(uuid.Nil.String()), Content: body, Type: "comment", ParentID: parentID})
		if createErr != nil {
			return createErr
		}
		comment = created.Comment()
		issueRevision = created.IssueRevision
		_, err = tx.Exec(ctx, `INSERT INTO linear_comment_link(workspace_id,binding_id,issue_id,comment_id,linear_comment_id,origin,remote_updated_at) VALUES($1,$2,$3,$4,$5,'linear',$6)`, b.WorkspaceID, b.ID, issueID, localID, remote.ID, remote.UpdatedAt)
	} else {
		updated, updateErr := q.UpdateComment(ctx, db.UpdateCommentParams{ID: localID, Content: body})
		if updateErr != nil {
			return updateErr
		}
		comment = updated.Comment()
		issueRevision = updated.IssueRevision
		_, err = tx.Exec(ctx, `UPDATE linear_comment_link SET remote_updated_at=$3,deleted=$4 WHERE binding_id=$1 AND comment_id=$2 AND workspace_id=$5`, b.ID, localID, remote.UpdatedAt, deleted, b.WorkspaceID)
	}
	if err != nil {
		return err
	}
	if err = tx.Commit(ctx); err != nil {
		return err
	}
	if w.bus != nil {
		eventType := protocol.EventCommentUpdated
		if newLink {
			eventType = protocol.EventCommentCreated
		}
		response := commentToResponse(comment, nil, nil)
		response.IssueRevision = issueRevision
		w.bus.Publish(events.Event{Type: eventType, WorkspaceID: uuidToString(b.WorkspaceID), ActorType: "system", ActorID: uuid.Nil.String(), Payload: map[string]any{"comment": response, "issue_revision": issueRevision}, TaskID: uuidToString(issueID)})
	}
	return nil
}

func (w *LinearWorker) handleCommentOutbox(ctx context.Context, c linearOutboxClaim, b workerBinding, token string) error {
	api, ok := w.api.(linearCommentAPI)
	if !ok {
		return errors.New("Linear comment API is unavailable")
	}
	var payload struct {
		CommentID  string `json:"comment_id"`
		Body       string `json:"body"`
		ParentID   string `json:"parent_id"`
		AuthorType string `json:"author_type"`
		AuthorID   string `json:"author_id"`
	}
	if err := json.Unmarshal(c.Payload, &payload); err != nil {
		return err
	}
	var remoteID, issueID, origin string
	if err := w.db.QueryRow(ctx, `SELECT cl.linear_comment_id,il.linear_issue_id,cl.origin FROM linear_comment_link cl JOIN linear_issue_link il ON il.binding_id=cl.binding_id AND il.patchbay_issue_id=cl.issue_id AND il.workspace_id=cl.workspace_id WHERE cl.binding_id=$1 AND cl.comment_id=$2 AND cl.workspace_id=$3 AND cl.issue_id=$4 AND il.sync_status<>'deleted'`, b.ID, parseUUID(payload.CommentID), b.WorkspaceID, c.IssueID).Scan(&remoteID, &issueID, &origin); err != nil {
		return err
	}
	if origin != "patchbay" {
		return nil
	}
	remote, found, err := api.FetchComment(ctx, token, remoteID)
	if err != nil {
		return err
	}
	if found && remote.Issue.ID != issueID {
		return errors.New("Linear outbound comment issue mismatch")
	}
	if c.EventType == "comment_deleted" {
		if found {
			return api.DeleteComment(ctx, token, remoteID)
		}
		return nil
	}
	if found {
		if remote.Body == payload.Body {
			return nil
		}
		return api.UpdateComment(ctx, token, remoteID, payload.Body)
	}
	parentID := ""
	if payload.ParentID != "" {
		if err = w.db.QueryRow(ctx, `SELECT linear_comment_id FROM linear_comment_link WHERE binding_id=$1 AND comment_id=$2 AND workspace_id=$3 AND issue_id=$4 AND NOT deleted`, b.ID, parseUUID(payload.ParentID), b.WorkspaceID, c.IssueID).Scan(&parentID); err != nil {
			return err
		}
	}
	author := "Patchbay " + payload.AuthorType
	var name string
	if payload.AuthorType == "member" {
		err = w.db.QueryRow(ctx, `SELECT u.name FROM "user" u JOIN member m ON m.user_id=u.id WHERE u.id=$1 AND m.workspace_id=$2`, parseUUID(payload.AuthorID), b.WorkspaceID).Scan(&name)
	} else if payload.AuthorType == "agent" {
		err = w.db.QueryRow(ctx, `SELECT name FROM agent WHERE id=$1 AND workspace_id=$2`, parseUUID(payload.AuthorID), b.WorkspaceID).Scan(&name)
	}
	if err != nil && !errors.Is(err, pgx.ErrNoRows) {
		return err
	}
	if name != "" {
		author = name + " via Patchbay"
	}
	_, err = api.CreateComment(ctx, token, remoteID, issueID, parentID, payload.Body, author)
	return err
}
