package handler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"strconv"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	linearapi "github.com/patchbay-ai/patchbay/server/internal/integrations/linear"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
)

const linearWorkerLease = 90 * time.Second

type LinearWorker struct {
	db           dbExecutor
	txStarter    txStarter
	box          *secretbox.Box
	api          linearapi.API
	clientID     string
	clientSecret string
	pullEnabled  bool
	pushEnabled  bool
	workerID     string
	wake         chan struct{}
	interval     time.Duration
	// pollInterval is the webhook-independent reconcile cadence. Linear
	// webhooks are the fast path, but a dropped delivery, a webhook that was
	// never registered, or a connection that was offline while the provider
	// retried all leave the local side permanently stale with nothing to
	// notice it. Every tick past this interval enqueues one durable
	// `binding_poll` per syncing binding, deduplicated by time bucket so
	// several replicas converge on a single poll rather than one each.
	pollInterval time.Duration
	nextPoll     time.Time
}

func NewLinearWorker(db dbExecutor, txs txStarter, box *secretbox.Box, api linearapi.API, clientID, clientSecret string, pull, push bool) *LinearWorker {
	return &LinearWorker{db: db, txStarter: txs, box: box, api: api, clientID: clientID, clientSecret: clientSecret, pullEnabled: pull, pushEnabled: push, workerID: uuid.NewString(), wake: make(chan struct{}, 1), interval: 30 * time.Second, pollInterval: 5 * time.Minute}
}

func (w *LinearWorker) Wake() {
	if w == nil {
		return
	}
	select {
	case w.wake <- struct{}{}:
	default:
	}
}
func (w *LinearWorker) Run(ctx context.Context) {
	if w == nil || w.db == nil || w.api == nil || w.box == nil {
		return
	}
	ticker := time.NewTicker(w.interval)
	defer ticker.Stop()
	w.Wake()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		case <-w.wake:
		}
		if w.pullEnabled && !time.Now().Before(w.nextPoll) {
			w.nextPoll = time.Now().Add(w.pollInterval)
			if err := w.enqueuePolls(ctx); err != nil {
				slog.ErrorContext(ctx, "linear poll enqueue failed", "error", err)
			}
		}
		for i := 0; i < 100; i++ {
			worked := false
			if w.pullEnabled {
				ok := w.processOneInbox(ctx)
				worked = worked || ok
			}
			if w.pushEnabled {
				ok := w.processOneOutbox(ctx)
				worked = worked || ok
			}
			if !worked {
				break
			}
		}
	}
}

// enqueuePolls writes one poll request per actively syncing binding into the
// same inbox the webhooks land in, so the fallback inherits the claim, retry,
// dead-letter and loop-suppression behaviour instead of running beside it. The
// delivery id carries a time bucket the width of the poll interval, so the
// inbox's (connection_id, delivery_id) uniqueness collapses concurrent
// enqueues from every replica into one row per binding per bucket.
func (w *LinearWorker) enqueuePolls(ctx context.Context) error {
	bucket := int64(w.pollInterval / time.Second)
	if bucket < 1 {
		bucket = 1
	}
	_, err := w.db.Exec(ctx, `INSERT INTO linear_sync_inbox(id,connection_id,delivery_id,event_type,payload)
SELECT gen_random_uuid(), b.connection_id,
       'poll:'||b.id::text||':'||(floor(extract(epoch FROM now())/$1))::bigint::text,
       'binding_poll',
       jsonb_build_object('binding_id', b.id)
FROM linear_project_binding b
JOIN linear_connection c ON c.id=b.connection_id
WHERE b.status='active' AND b.sync_mode IN ('import','two_way') AND c.status='active'
ON CONFLICT (connection_id,delivery_id) DO NOTHING`, bucket)
	return err
}

type linearClaim struct {
	ID, ConnectionID      pgtype.UUID
	DeliveryID, EventType string
	Payload               []byte
	Attempts, MaxAttempts int32
}

func (w *LinearWorker) claimInbox(ctx context.Context) (linearClaim, bool, error) {
	var c linearClaim
	err := w.db.QueryRow(ctx, `WITH candidate AS (SELECT id FROM linear_sync_inbox WHERE processed_at IS NULL AND dead_lettered_at IS NULL AND available_at<=now() AND (locked_until IS NULL OR locked_until<now()) ORDER BY received_at FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE linear_sync_inbox i SET locked_by=$1,locked_until=now()+make_interval(secs => $2),attempts=attempts+1 FROM candidate WHERE i.id=candidate.id RETURNING i.id,i.connection_id,i.delivery_id,i.event_type,i.payload,i.attempts,i.max_attempts`, w.workerID, int(linearWorkerLease/time.Second)).Scan(&c.ID, &c.ConnectionID, &c.DeliveryID, &c.EventType, &c.Payload, &c.Attempts, &c.MaxAttempts)
	if errors.Is(err, pgx.ErrNoRows) {
		return c, false, nil
	}
	return c, err == nil, err
}
func (w *LinearWorker) processOneInbox(ctx context.Context) bool {
	c, ok, err := w.claimInbox(ctx)
	if err != nil {
		slog.ErrorContext(ctx, "linear inbox claim failed", "error", err)
		return false
	}
	if !ok {
		return false
	}
	err = w.handleInbox(ctx, c)
	w.finish(ctx, "linear_sync_inbox", c.ID, c.ConnectionID, c.Attempts, c.MaxAttempts, err)
	return true
}

type linearOutboxClaim struct {
	ID, WorkspaceID, BindingID, IssueID pgtype.UUID
	EventType                           string
	Payload                             []byte
	Attempts, MaxAttempts               int32
}

func (w *LinearWorker) claimOutbox(ctx context.Context) (linearOutboxClaim, bool, error) {
	var c linearOutboxClaim
	// The `older` guard keeps outbound work FIFO per (binding, issue): only
	// the oldest unfinished event for an issue is ever claimable. Without it
	// two workers can hold `issue_created` and `issue_updated` for the same
	// issue at once, which both creates the remote issue twice (neither
	// claim can see the link the other is about to write) and lets the
	// update land before the create. A row waiting on backoff deliberately
	// blocks its successors: applying a later revision first would publish a
	// state the local side has already moved past.
	err := w.db.QueryRow(ctx, `WITH candidate AS (
	SELECT o.id FROM linear_sync_outbox o
	WHERE o.processed_at IS NULL AND o.dead_lettered_at IS NULL AND o.available_at<=now()
	  AND (o.locked_until IS NULL OR o.locked_until<now())
	  AND NOT EXISTS (
	    SELECT 1 FROM linear_sync_outbox older
	    WHERE older.binding_id=o.binding_id AND older.issue_id=o.issue_id
	      AND older.processed_at IS NULL AND older.dead_lettered_at IS NULL
	      AND (older.created_at, older.id) < (o.created_at, o.id))
	ORDER BY o.created_at FOR UPDATE SKIP LOCKED LIMIT 1)
UPDATE linear_sync_outbox o SET locked_by=$1,locked_until=now()+make_interval(secs => $2),attempts=attempts+1,updated_at=now() FROM candidate WHERE o.id=candidate.id RETURNING o.id,o.workspace_id,o.binding_id,o.issue_id,o.event_type,o.payload,o.attempts,o.max_attempts`, w.workerID, int(linearWorkerLease/time.Second)).Scan(&c.ID, &c.WorkspaceID, &c.BindingID, &c.IssueID, &c.EventType, &c.Payload, &c.Attempts, &c.MaxAttempts)
	if errors.Is(err, pgx.ErrNoRows) {
		return c, false, nil
	}
	return c, err == nil, err
}
func (w *LinearWorker) processOneOutbox(ctx context.Context) bool {
	c, ok, err := w.claimOutbox(ctx)
	if err != nil {
		slog.ErrorContext(ctx, "linear outbox claim failed", "error", err)
		return false
	}
	if !ok {
		return false
	}
	err = w.handleOutbox(ctx, c)
	var connectionID pgtype.UUID
	if err != nil {
		// Push failures are connection health just as much as pull failures
		// are; without this the workspace's Linear panel keeps reporting a
		// healthy connection while nothing it publishes is landing.
		_ = w.db.QueryRow(ctx, `SELECT connection_id FROM linear_project_binding WHERE id=$1`, c.BindingID).Scan(&connectionID)
	}
	w.finish(ctx, "linear_sync_outbox", c.ID, connectionID, c.Attempts, c.MaxAttempts, err)
	return true
}

func retryDelay(attempt int32) time.Duration {
	seconds := math.Pow(2, float64(max(attempt-1, 0)))
	return time.Duration(min(seconds, 900)) * time.Second
}
func (w *LinearWorker) finish(ctx context.Context, table string, id, connectionID pgtype.UUID, attempts, maxAttempts int32, processErr error) {
	if processErr == nil {
		_, _ = w.db.Exec(ctx, `UPDATE `+table+` SET processed_at=now(),locked_by=NULL,locked_until=NULL,last_error=NULL`+func() string {
			if table == "linear_sync_outbox" {
				return `,updated_at=now()`
			}
			return ``
		}()+` WHERE id=$1`, id)
		if connectionID.Valid {
			_, _ = w.db.Exec(ctx, `UPDATE linear_connection SET last_success_at=now(),last_error=NULL,updated_at=now() WHERE id=$1`, connectionID)
		}
		return
	}
	dead := attempts >= maxAttempts
	_, _ = w.db.Exec(ctx, `UPDATE `+table+` SET available_at=now()+make_interval(secs => $2),locked_by=NULL,locked_until=NULL,last_error=$3,dead_lettered_at=CASE WHEN $4 THEN now() ELSE NULL END`+func() string {
		if table == "linear_sync_outbox" {
			return `,updated_at=now()`
		}
		return ``
	}()+` WHERE id=$1`, id, int(retryDelay(attempts)/time.Second), processErr.Error(), dead)
	if connectionID.Valid {
		_, _ = w.db.Exec(ctx, `UPDATE linear_connection SET last_error=$2,updated_at=now() WHERE id=$1`, connectionID, processErr.Error())
	}
	slog.WarnContext(ctx, "linear sync item failed", "queue", table, "id", uuidToString(id), "attempt", attempts, "dead_lettered", dead, "error", processErr)
}

type workerBinding struct {
	ID, WorkspaceID, ConnectionID, ProjectID, CreatorID pgtype.UUID
	LinearProjectID                                     string
	TeamID                                              pgtype.Text
	Mode                                                string
	StatusMapping, AgentMapping                         map[string]any
}

func (w *LinearWorker) loadBinding(ctx context.Context, id pgtype.UUID) (workerBinding, error) {
	var b workerBinding
	var sm, am []byte
	err := w.db.QueryRow(ctx, `SELECT id,workspace_id,connection_id,patchbay_project_id,created_by_id,linear_project_id,linear_team_id,sync_mode,status_mapping,agent_label_mapping FROM linear_project_binding WHERE id=$1 AND status='active'`, id).Scan(&b.ID, &b.WorkspaceID, &b.ConnectionID, &b.ProjectID, &b.CreatorID, &b.LinearProjectID, &b.TeamID, &b.Mode, &sm, &am)
	_ = json.Unmarshal(sm, &b.StatusMapping)
	_ = json.Unmarshal(am, &b.AgentMapping)
	return b, err
}
func (w *LinearWorker) bindingForRemote(ctx context.Context, connectionID pgtype.UUID, projectID, teamID string) (workerBinding, error) {
	var id pgtype.UUID
	err := w.db.QueryRow(ctx, `SELECT id FROM linear_project_binding WHERE connection_id=$1 AND status='active' AND sync_mode IN ('import','two_way') AND linear_project_id=$2 AND (linear_team_id IS NULL OR linear_team_id=$3) ORDER BY created_at LIMIT 1`, connectionID, projectID, teamID).Scan(&id)
	if err != nil {
		return workerBinding{}, err
	}
	return w.loadBinding(ctx, id)
}

func (w *LinearWorker) accessToken(ctx context.Context, connectionID pgtype.UUID) (string, error) {
	var access, refresh []byte
	var expires time.Time
	var status string
	err := w.db.QueryRow(ctx, `SELECT access_token_encrypted,refresh_token_encrypted,token_expires_at,status FROM linear_connection WHERE id=$1`, connectionID).Scan(&access, &refresh, &expires, &status)
	if err != nil {
		return "", err
	}
	if status != "active" {
		return "", fmt.Errorf("linear connection status %s", status)
	}
	plain, err := w.box.Open(access)
	if err != nil {
		return "", err
	}
	if time.Until(expires) > 2*time.Minute {
		return string(plain), nil
	}
	refreshPlain, err := w.box.Open(refresh)
	if err != nil {
		return "", err
	}
	token, err := w.api.RefreshToken(ctx, string(refreshPlain), w.clientID, w.clientSecret)
	if err != nil {
		_, _ = w.db.Exec(ctx, `UPDATE linear_connection SET status='reauthorization_required',last_error=$2,updated_at=now() WHERE id=$1`, connectionID, err.Error())
		return "", err
	}
	if token.ExpiresIn <= 0 {
		token.ExpiresIn = 30 * 24 * time.Hour
	}
	newAccess, err := w.box.Seal([]byte(token.AccessToken))
	if err != nil {
		return "", err
	}
	newRefresh, err := w.box.Seal([]byte(token.RefreshToken))
	if err != nil {
		return "", err
	}
	_, err = w.db.Exec(ctx, `UPDATE linear_connection SET access_token_encrypted=$2,refresh_token_encrypted=$3,token_expires_at=$4,scopes=CASE WHEN $5='' THEN scopes ELSE to_jsonb(regexp_split_to_array($5,'[, ]+')) END,last_error=NULL,updated_at=now() WHERE id=$1`, connectionID, newAccess, newRefresh, time.Now().Add(token.ExpiresIn), token.Scope)
	return token.AccessToken, err
}

type webhookIssue struct {
	ID          string    `json:"id"`
	Identifier  string    `json:"identifier"`
	Title       string    `json:"title"`
	Description *string   `json:"description"`
	Priority    int       `json:"priority"`
	UpdatedAt   time.Time `json:"updatedAt"`
	Project     *struct {
		ID string `json:"id"`
	} `json:"project"`
	Team *struct {
		ID string `json:"id"`
	} `json:"team"`
	State *struct {
		ID   string `json:"id"`
		Type string `json:"type"`
	} `json:"state"`
	Assignee *struct {
		ID string `json:"id"`
	} `json:"assignee"`
}
type webhookEnvelope struct {
	Action, Type     string
	Data             webhookIssue `json:"data"`
	WebhookTimestamp int64        `json:"webhookTimestamp"`
}

func remoteFromWebhook(e webhookEnvelope) linearapi.Issue {
	i := linearapi.Issue{ID: e.Data.ID, Identifier: e.Data.Identifier, Title: e.Data.Title, Priority: e.Data.Priority, UpdatedAt: e.Data.UpdatedAt}
	if e.Data.Description != nil {
		i.Description = *e.Data.Description
	}
	if e.Data.Project != nil {
		i.ProjectID = e.Data.Project.ID
	}
	if e.Data.Team != nil {
		i.TeamID = e.Data.Team.ID
	}
	if e.Data.State != nil {
		i.StateID, i.StateType = e.Data.State.ID, e.Data.State.Type
	}
	if e.Data.Assignee != nil {
		i.AssigneeID = e.Data.Assignee.ID
	}
	i.Deleted = e.Action == "remove" || e.Action == "delete"
	return i
}

// importBinding is the list-and-apply path shared by the operator-triggered
// initial import and the periodic poll fallback. applyRemote is keyed on the
// remote issue's own updatedAt, so re-listing a project whose issues have not
// moved converges on no local write at all — which is what makes it safe to
// run this on a timer next to the webhooks.
func (w *LinearWorker) importBinding(ctx context.Context, payload []byte, eventPrefix string) error {
	var p struct {
		BindingID string `json:"binding_id"`
	}
	if err := json.Unmarshal(payload, &p); err != nil {
		return err
	}
	bid, err := uuid.Parse(p.BindingID)
	if err != nil {
		return err
	}
	b, err := w.loadBinding(ctx, pgtype.UUID{Bytes: bid, Valid: true})
	if errors.Is(err, pgx.ErrNoRows) {
		// The binding was paused or unbound after the request was queued.
		// Retrying cannot make it reappear, so acknowledge instead of
		// burning the attempt budget down to a dead letter.
		return nil
	}
	if err != nil {
		return err
	}
	token, err := w.accessToken(ctx, b.ConnectionID)
	if err != nil {
		return err
	}
	issues, err := w.api.ListIssues(ctx, token, b.LinearProjectID, b.TeamID.String)
	if err != nil {
		return err
	}
	for _, issue := range issues {
		// A listed issue has no delivery of its own, so its identity is the
		// remote revision itself: re-listing an untouched issue yields the
		// same id and stops at the already-applied check, while a genuinely
		// edited one yields a new id and is applied.
		eventID := eventPrefix + issue.ID + ":" + strconv.FormatInt(issue.UpdatedAt.UnixMilli(), 10)
		if err := w.applyRemote(ctx, b, issue, eventID, issue.UpdatedAt.UnixMilli()); err != nil {
			return err
		}
	}
	return nil
}

func (w *LinearWorker) handleInbox(ctx context.Context, c linearClaim) error {
	switch c.EventType {
	case "initial_import":
		return w.importBinding(ctx, c.Payload, "initial-import:")
	case "binding_poll":
		return w.importBinding(ctx, c.Payload, "poll:")
	}
	var e webhookEnvelope
	if err := json.Unmarshal(c.Payload, &e); err != nil {
		return err
	}
	if e.Type != "Issue" && e.Type != "issue" {
		return nil
	}
	remote := remoteFromWebhook(e)
	b, err := w.bindingForRemote(ctx, c.ConnectionID, remote.ProjectID, remote.TeamID)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil
	}
	if err != nil {
		return err
	}
	return w.applyRemote(ctx, b, remote, "webhook:"+c.DeliveryID, e.WebhookTimestamp)
}

func remoteStatus(b workerBinding, i linearapi.Issue) string {
	if v, ok := b.StatusMapping[i.StateID].(string); ok && v != "" {
		return v
	}
	switch i.StateType {
	case "completed":
		return "done"
	case "canceled", "cancelled":
		return "cancelled"
	case "started":
		return "in_progress"
	case "unstarted":
		return "todo"
	default:
		return "backlog"
	}
}
func remotePriority(v int) string {
	switch v {
	case 1:
		return "urgent"
	case 2:
		return "high"
	case 3:
		return "medium"
	case 4:
		return "low"
	default:
		return "none"
	}
}
func snapshot(i linearapi.Issue, b workerBinding) map[string]any {
	return map[string]any{"title": i.Title, "description": i.Description, "status": remoteStatus(b, i), "priority": remotePriority(i.Priority)}
}
func valueEqual(a, b any) bool {
	x, _ := json.Marshal(a)
	y, _ := json.Marshal(b)
	return string(x) == string(y)
}
func (w *LinearWorker) applyRemote(ctx context.Context, b workerBinding, remote linearapi.Issue, eventID string, eventAt int64) error {
	tx, err := w.txStarter.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx)
	if _, err = tx.Exec(ctx, `SELECT set_config('patchbay.linear_remote_apply','on',true)`); err != nil {
		return err
	}
	var linkID, issueID pgtype.UUID
	var baseRaw []byte
	var seenAt pgtype.Int8
	var seenEvent pgtype.Text
	err = tx.QueryRow(ctx, `SELECT id,patchbay_issue_id,last_common_snapshot,last_remote_event_at_ms,last_remote_event_id FROM linear_issue_link WHERE binding_id=$1 AND linear_issue_id=$2 FOR UPDATE`, b.ID, remote.ID).Scan(&linkID, &issueID, &baseRaw, &seenAt, &seenEvent)
	if errors.Is(err, pgx.ErrNoRows) {
		if remote.Deleted {
			return tx.Commit(ctx)
		}
		var number int32
		if err = tx.QueryRow(ctx, `UPDATE workspace SET issue_counter=issue_counter+1 WHERE id=$1 RETURNING issue_counter`, b.WorkspaceID).Scan(&number); err != nil {
			return err
		}
		var position float64
		_ = tx.QueryRow(ctx, `SELECT COALESCE(MIN(position)-1,0) FROM issue WHERE workspace_id=$1 AND status=$2`, b.WorkspaceID, remoteStatus(b, remote)).Scan(&position)
		issueID = parseUUID(uuid.NewString())
		origin, parseErr := uuid.Parse(remote.ID)
		if parseErr != nil {
			return fmt.Errorf("linear issue id is not UUID: %w", parseErr)
		}
		_, err = tx.Exec(ctx, `INSERT INTO issue(id,workspace_id,title,description,status,priority,creator_type,creator_id,position,number,project_id,origin_type,origin_id,last_activity_at) VALUES($1,$2,$3,$4,$5,$6,'member',$7,$8,$9,$10,'linear',$11,now())`, issueID, b.WorkspaceID, remote.Title, remote.Description, remoteStatus(b, remote), remotePriority(remote.Priority), b.CreatorID, position, number, b.ProjectID, pgtype.UUID{Bytes: origin, Valid: true})
		if err != nil {
			return err
		}
		linkID = parseUUID(uuid.NewString())
		snap, _ := json.Marshal(snapshot(remote, b))
		_, err = tx.Exec(ctx, `INSERT INTO linear_issue_link(id,workspace_id,binding_id,patchbay_issue_id,linear_issue_id,linear_identifier,last_common_snapshot,remote_updated_at,last_remote_event_at_ms,last_remote_event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`, linkID, b.WorkspaceID, b.ID, issueID, remote.ID, remote.Identifier, snap, remote.UpdatedAt, eventAt, eventID)
		if err != nil {
			return err
		}
		return tx.Commit(ctx)
	}
	if err != nil {
		return err
	}
	// Redeliveries and out-of-order deliveries are both normal for a webhook
	// feed, and the poll fallback deliberately re-lists issues it has already
	// imported. Replaying an event the link has already absorbed would take
	// the already-merged remote value as a fresh remote change and could
	// resurrect a conflict that was just resolved, so the link's high-water
	// mark decides what is actually new.
	if seenEvent.Valid && seenEvent.String == eventID {
		return tx.Commit(ctx)
	}
	if eventAt > 0 && seenAt.Valid && seenAt.Int64 > eventAt {
		return tx.Commit(ctx)
	}
	if remote.Deleted {
		_, err = tx.Exec(ctx, `UPDATE issue SET status='cancelled',revision=revision+1,updated_at=now(),last_activity_at=now() WHERE id=$1 AND workspace_id=$2`, issueID, b.WorkspaceID)
		if err == nil {
			_, err = tx.Exec(ctx, `UPDATE linear_issue_link SET sync_status='deleted',last_remote_event_at_ms=$2,last_remote_event_id=$3,updated_at=now() WHERE id=$1`, linkID, eventAt, eventID)
		}
		if err != nil {
			return err
		}
		return tx.Commit(ctx)
	}
	var title, status, priority string
	var description pgtype.Text
	if err = tx.QueryRow(ctx, `SELECT title,description,status,priority FROM issue WHERE id=$1 AND workspace_id=$2 FOR UPDATE`, issueID, b.WorkspaceID).Scan(&title, &description, &status, &priority); err != nil {
		return err
	}
	local := map[string]any{"title": title, "description": description.String, "status": status, "priority": priority}
	var base map[string]any
	_ = json.Unmarshal(baseRaw, &base)
	incoming := snapshot(remote, b)
	next := map[string]any{}
	conflicted := false
	for _, field := range []string{"title", "description", "status", "priority"} {
		localChanged := !valueEqual(local[field], base[field])
		remoteChanged := !valueEqual(incoming[field], base[field])
		if localChanged && remoteChanged && !valueEqual(local[field], incoming[field]) {
			conflicted = true
			rawBase, _ := json.Marshal(base[field])
			rawLocal, _ := json.Marshal(local[field])
			rawRemote, _ := json.Marshal(incoming[field])
			_, err = tx.Exec(ctx, `INSERT INTO linear_sync_conflict(id,workspace_id,binding_id,link_id,patchbay_issue_id,linear_issue_id,field,base_value,local_value,remote_value,source_event_id,source_event_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT (link_id,field) WHERE status='open' DO UPDATE SET local_value=EXCLUDED.local_value,remote_value=EXCLUDED.remote_value,source_event_id=EXCLUDED.source_event_id,source_event_at_ms=EXCLUDED.source_event_at_ms,updated_at=now()`, parseUUID(uuid.NewString()), b.WorkspaceID, b.ID, linkID, issueID, remote.ID, field, rawBase, rawLocal, rawRemote, eventID, eventAt)
			if err != nil {
				return err
			}
			next[field] = local[field]
		} else if remoteChanged {
			next[field] = incoming[field]
		} else {
			next[field] = local[field]
		}
	}
	// The echo of our own push arrives as an ordinary remote event carrying
	// exactly the values we just wrote, so `next` comes back equal to the
	// local row. Writing it anyway would bump revision and last_activity_at
	// on every round trip — visible to users as phantom activity, and enough
	// to re-enter the outbox the moment the remote-apply guard is ever
	// missed. Only a real difference gets written.
	if !valueEqual(next, local) {
		_, err = tx.Exec(ctx, `UPDATE issue SET title=$3,description=$4,status=$5,priority=$6,revision=revision+1,updated_at=now(),last_activity_at=now() WHERE id=$1 AND workspace_id=$2`, issueID, b.WorkspaceID, next["title"], next["description"], next["status"], next["priority"])
		if err != nil {
			return err
		}
	}
	snap, _ := json.Marshal(incoming)
	syncStatus := "active"
	if conflicted {
		syncStatus = "conflict"
	}
	_, err = tx.Exec(ctx, `UPDATE linear_issue_link SET linear_identifier=$2,last_common_snapshot=$3,remote_updated_at=$4,last_remote_event_at_ms=$5,last_remote_event_id=$6,sync_status=$7,updated_at=now() WHERE id=$1`, linkID, remote.Identifier, snap, remote.UpdatedAt, eventAt, eventID, syncStatus)
	if err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func localPriority(v string) int {
	switch v {
	case "urgent":
		return 1
	case "high":
		return 2
	case "medium":
		return 3
	case "low":
		return 4
	default:
		return 0
	}
}
func stateForLocal(b workerBinding, status string) string {
	for remote, local := range b.StatusMapping {
		if s, ok := local.(string); ok && s == status {
			return remote
		}
	}
	return ""
}
func (w *LinearWorker) handleOutbox(ctx context.Context, c linearOutboxClaim) error {
	b, err := w.loadBinding(ctx, c.BindingID)
	if errors.Is(err, pgx.ErrNoRows) {
		// Pausing or deleting a binding is how an operator stops publishing.
		// Events queued before that moment must not keep retrying until they
		// dead-letter and show up as integration failures.
		return nil
	}
	if err != nil {
		return err
	}
	if b.WorkspaceID != c.WorkspaceID {
		return errors.New("linear outbox workspace mismatch")
	}
	if b.Mode != "publish" && b.Mode != "two_way" {
		return nil
	}
	token, err := w.accessToken(ctx, b.ConnectionID)
	if err != nil {
		return err
	}
	var linkID pgtype.UUID
	var remoteID string
	linkErr := w.db.QueryRow(ctx, `SELECT id,linear_issue_id FROM linear_issue_link WHERE binding_id=$1 AND patchbay_issue_id=$2 AND sync_status<>'deleted'`, b.ID, c.IssueID).Scan(&linkID, &remoteID)
	if c.EventType == "issue_deleted" {
		if errors.Is(linkErr, pgx.ErrNoRows) {
			return nil
		}
		if linkErr != nil {
			return linkErr
		}
		if err = w.api.DeleteIssue(ctx, token, remoteID); err != nil {
			return err
		}
		_, err = w.db.Exec(ctx, `UPDATE linear_issue_link SET sync_status='deleted',updated_at=now() WHERE id=$1 AND workspace_id=$2`, linkID, b.WorkspaceID)
		return err
	}
	var title, status, priority string
	var description pgtype.Text
	if err = w.db.QueryRow(ctx, `SELECT title,description,status,priority FROM issue WHERE id=$1 AND workspace_id=$2`, c.IssueID, b.WorkspaceID).Scan(&title, &description, &status, &priority); err != nil {
		return err
	}
	var assignee string
	_ = w.db.QueryRow(ctx, `SELECT mb.linear_user_id FROM issue i JOIN linear_member_binding mb ON mb.workspace_id=i.workspace_id AND mb.patchbay_user_id=i.executor_id WHERE i.id=$1 AND i.workspace_id=$2`, c.IssueID, b.WorkspaceID).Scan(&assignee)
	input := linearapi.IssueInput{TeamID: b.TeamID.String, ProjectID: b.LinearProjectID, Title: title, Description: description.String, Priority: localPriority(priority), StateID: stateForLocal(b, status), AssigneeID: assignee}
	var remote linearapi.Issue
	if errors.Is(linkErr, pgx.ErrNoRows) {
		remote, err = w.api.CreateIssue(ctx, token, input)
		if err != nil {
			return err
		}
		// The snapshot recorded here is the local state we just published, so
		// the provider's echo of this create compares equal to the base and
		// applies nothing locally. Getting this wrong is what turns a one-way
		// publish into a sync loop.
		snap, _ := json.Marshal(map[string]any{"title": title, "description": description.String, "status": status, "priority": priority})
		// The arbiter is uq_linear_issue_link_local, the only unique index
		// covering this pair; naming a set no index covers made every create
		// fail with "no unique or exclusion constraint matching the ON
		// CONFLICT specification" after the remote issue had already been
		// created.
		tag, insertErr := w.db.Exec(ctx, `INSERT INTO linear_issue_link(id,workspace_id,binding_id,patchbay_issue_id,linear_issue_id,linear_identifier,last_common_snapshot,remote_updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (workspace_id,patchbay_issue_id) WHERE sync_status<>'deleted' DO NOTHING`, parseUUID(uuid.NewString()), b.WorkspaceID, b.ID, c.IssueID, remote.ID, remote.Identifier, snap, remote.UpdatedAt)
		if insertErr != nil {
			return insertErr
		}
		if tag.RowsAffected() == 0 {
			slog.WarnContext(ctx, "linear create raced an existing issue link", "issue_id", uuidToString(c.IssueID), "linear_issue_id", remote.ID)
		}
		return nil
	}
	if linkErr != nil {
		return linkErr
	}
	remote, err = w.api.UpdateIssue(ctx, token, remoteID, input)
	if err != nil {
		return err
	}
	snap, _ := json.Marshal(map[string]any{"title": title, "description": description.String, "status": status, "priority": priority})
	_, err = w.db.Exec(ctx, `UPDATE linear_issue_link SET linear_identifier=$2,last_common_snapshot=$3,remote_updated_at=$4,sync_status='active',updated_at=now() WHERE id=$1 AND workspace_id=$5`, linkID, remote.Identifier, snap, remote.UpdatedAt, b.WorkspaceID)
	return err
}
