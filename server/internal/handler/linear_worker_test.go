package handler

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/linear"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
)

// fakeLinearAPI stands in for Linear's GraphQL and OAuth endpoints. Every call
// is recorded so the tests can assert what the worker decided to send, which is
// the half of bilateral sync no database assertion can see.
type fakeLinearAPI struct {
	mu           sync.Mutex
	listed       []linear.Issue
	created      []linear.IssueInput
	updated      []linear.IssueInput
	deleted      []string
	attached     []string
	detached     []string
	revoked      []string
	refresh      linear.Token
	refreshN     int
	commentLists int
	err          error
	authErr      error
}

func (f *fakeLinearAPI) ListComments(context.Context, string, string) ([]linear.Comment, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.commentLists++
	return nil, f.err
}
func (f *fakeLinearAPI) FetchComment(context.Context, string, string) (linear.Comment, bool, error) {
	return linear.Comment{}, false, f.err
}
func (f *fakeLinearAPI) CreateComment(context.Context, string, string, string, string, string, string) (linear.Comment, error) {
	return linear.Comment{}, errors.New("comment create not configured in fixture")
}
func (f *fakeLinearAPI) UpdateComment(context.Context, string, string, string) error {
	return errors.New("comment update not configured in fixture")
}
func (f *fakeLinearAPI) DeleteComment(context.Context, string, string) error {
	return errors.New("comment delete not configured in fixture")
}

func (f *fakeLinearAPI) ExchangeAuthorizationCode(context.Context, string, string, string, string, string) (linear.Token, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.authErr != nil {
		return linear.Token{}, f.authErr
	}
	return f.refresh, nil
}

func (f *fakeLinearAPI) RefreshToken(context.Context, string, string, string) (linear.Token, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.refreshN++
	if f.authErr != nil {
		return linear.Token{}, f.authErr
	}
	return f.refresh, nil
}

func (f *fakeLinearAPI) RevokeToken(_ context.Context, token, _, _ string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.revoked = append(f.revoked, token)
	return f.authErr
}

func (f *fakeLinearAPI) ListIssues(context.Context, string, string, string) ([]linear.Issue, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.listed, f.err
}

func (f *fakeLinearAPI) DiscoverIdentity(context.Context, string) (linear.Identity, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.authErr != nil {
		return linear.Identity{}, f.authErr
	}
	return linear.Identity{ID: "viewer", OrganizationID: "org", OrganizationName: "Test org", ActorID: "viewer"}, nil
}

func (f *fakeLinearAPI) Catalog(context.Context, string) (linear.Catalog, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.authErr != nil {
		return linear.Catalog{}, f.authErr
	}
	return linear.Catalog{}, nil
}

func (f *fakeLinearAPI) ValidateBinding(context.Context, string, string, string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.authErr
}

func (f *fakeLinearAPI) DryRunCounts(context.Context, string, string, string, map[string]any) (linear.DryRunCounts, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.authErr != nil {
		return linear.DryRunCounts{}, f.authErr
	}
	return linear.DryRunCounts{RemoteIssues: len(f.listed)}, nil
}

func (f *fakeLinearAPI) FetchIssue(_ context.Context, _ string, id string) (linear.Issue, bool, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.err != nil {
		return linear.Issue{}, false, f.err
	}
	for _, issue := range f.listed {
		if issue.ID == id {
			return issue, true, nil
		}
	}
	return linear.Issue{}, false, nil
}

func (f *fakeLinearAPI) CreateIssue(_ context.Context, _ string, in linear.IssueInput) (linear.Issue, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.created = append(f.created, in)
	issue := linear.Issue{ID: "10000000-0000-0000-0000-000000000001", Identifier: "ENG-1", Title: in.Title, Description: in.Description, Priority: in.Priority, DueDate: in.DueDate, UpdatedAt: time.Now(), ProjectID: in.ProjectID, TeamID: in.TeamID}
	if in.AssigneeID != nil {
		issue.AssigneeID = *in.AssigneeID
	}
	f.listed = append(f.listed, issue)
	return issue, f.err
}

func (f *fakeLinearAPI) UpdateIssue(_ context.Context, _ string, id string, in linear.IssueInput) (linear.Issue, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.updated = append(f.updated, in)
	issue := linear.Issue{ID: id, Identifier: "ENG-1", Title: in.Title, Description: in.Description, Priority: in.Priority, DueDate: in.DueDate, UpdatedAt: time.Now(), ProjectID: in.ProjectID, TeamID: in.TeamID}
	if in.AssigneeID != nil {
		issue.AssigneeID = *in.AssigneeID
	}
	for index := range f.listed {
		if f.listed[index].ID == id {
			f.listed[index] = issue
		}
	}
	return issue, f.err
}

func (f *fakeLinearAPI) DeleteIssue(_ context.Context, _ string, id string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.deleted = append(f.deleted, id)
	return f.err
}

func (f *fakeLinearAPI) UpsertAttachment(_ context.Context, _, issueID, title, rawURL string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.attached = append(f.attached, issueID+"|"+title+"|"+rawURL)
	return f.err
}

func (f *fakeLinearAPI) DeleteAttachmentByURL(_ context.Context, _, issueID, rawURL string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.detached = append(f.detached, issueID+"|"+rawURL)
	return f.err
}

func (f *fakeLinearAPI) calls() (created, updated, deleted, refreshed int) {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.created), len(f.updated), len(f.deleted), f.refreshN
}

type linearFixture struct {
	worker       *LinearWorker
	box          *secretbox.Box
	api          *fakeLinearAPI
	bindingID    string
	projectID    string
	connectionID string
}

// setupLinearWorker builds one connection plus one binding in the shared test
// workspace and returns a worker wired to them.
//
// Teardown deletes the local issues before the queues, not after: `issue` runs
// the outbound trigger, so a teardown that drained the outbox first would
// refill it on its way out and leave rows the next test's claim can reach.
func setupLinearWorker(t *testing.T, mode string, api *fakeLinearAPI) linearFixture {
	t.Helper()
	if testPool == nil {
		t.Skip("database not available")
	}
	box, err := secretbox.New(make([]byte, secretbox.KeySize))
	if err != nil {
		t.Fatal(err)
	}
	sealedAccess, err := box.Seal([]byte("access"))
	if err != nil {
		t.Fatal(err)
	}
	sealedRefresh, err := box.Seal([]byte("refresh"))
	if err != nil {
		t.Fatal(err)
	}
	purgeLinearState(t)
	projectID := dbfx.Project(t, "Linear worker project")
	connectionID := dbfx.Insert(t, "linear_connection", testutil.Cols{
		"workspace_id":            testWorkspaceID,
		"organization_id":         "org-" + time.Now().Format("150405.000000000"),
		"organization_name":       "Test org",
		"actor_id":                "actor",
		"access_token_encrypted":  sealedAccess,
		"refresh_token_encrypted": sealedRefresh,
		"token_expires_at":        testutil.Raw("now() + interval '1 hour'"),
		"scopes":                  testutil.Raw("'[]'::jsonb"),
		"created_by_id":           testUserID,
	})
	bindingID := dbfx.Insert(t, "linear_project_binding", testutil.Cols{
		"workspace_id":            testWorkspaceID,
		"connection_id":           connectionID,
		"patchbay_project_id":     projectID,
		"linear_project_id":       "linear-project",
		"linear_team_id":          "linear-team",
		"status":                  "active",
		"sync_mode":               mode,
		"initial_source_of_truth": map[string]string{"import": "linear", "publish": "patchbay", "two_way": "linear", "not_synced": "patchbay"}[mode],
		"status_mapping":          testutil.Raw(`'{"remote-todo":"todo","remote-doing":"in_progress"}'::jsonb`),
		"agent_label_mapping":     testutil.Raw("'{}'::jsonb"),
		"created_by_id":           testUserID,
	})
	t.Cleanup(func() { purgeLinearState(t) })
	worker := NewLinearWorker(testPool, testPool, box, api, "client", "secret", true, true)
	return linearFixture{worker: worker, box: box, api: api, bindingID: bindingID, projectID: projectID, connectionID: connectionID}
}

func purgeLinearState(t *testing.T) {
	t.Helper()
	ctx := context.Background()
	for _, stmt := range []string{
		`DELETE FROM issue WHERE workspace_id=$1 AND origin_type='linear'`,
		`DELETE FROM linear_sync_conflict WHERE workspace_id=$1`,
		`DELETE FROM linear_comment_link WHERE workspace_id=$1`,
		`DELETE FROM linear_issue_link WHERE workspace_id=$1`,
		`DELETE FROM linear_project_binding WHERE workspace_id=$1`,
		`DELETE FROM linear_sync_outbox WHERE workspace_id=$1`,
		`DELETE FROM linear_sync_inbox WHERE connection_id IN (SELECT id FROM linear_connection WHERE workspace_id=$1)`,
		`DELETE FROM linear_member_binding WHERE workspace_id=$1`,
		`DELETE FROM linear_connection WHERE workspace_id=$1`,
	} {
		if _, err := testPool.Exec(ctx, stmt, testWorkspaceID); err != nil {
			t.Fatalf("purge linear state: %v (%s)", err, stmt)
		}
	}
}

func webhookPayload(t *testing.T, action string, at int64, issue map[string]any) []byte {
	t.Helper()
	body, err := json.Marshal(map[string]any{"type": "Issue", "action": action, "webhookTimestamp": at, "data": issue})
	if err != nil {
		t.Fatal(err)
	}
	return body
}

func (f linearFixture) queueWebhook(t *testing.T, delivery string, body []byte) string {
	t.Helper()
	var envelope webhookEnvelope
	if json.Unmarshal(body, &envelope) == nil && envelope.Data.ID != "" && f.api != nil {
		remote := remoteFromWebhook(envelope)
		f.api.mu.Lock()
		found := false
		for index := range f.api.listed {
			if f.api.listed[index].ID == remote.ID {
				f.api.listed[index] = remote
				found = true
			}
		}
		if !found && !remote.Deleted {
			f.api.listed = append(f.api.listed, remote)
		}
		f.api.mu.Unlock()
	}
	return dbfx.Insert(t, "linear_sync_inbox", testutil.Cols{
		"connection_id": f.connectionID,
		"delivery_id":   delivery,
		"event_type":    "Issue:update",
		"payload":       body,
	})
}

// linearTestSignature reproduces the HMAC the webhook endpoint verifies.
func linearTestSignature(t *testing.T, secret string, body []byte) string {
	t.Helper()
	mac := hmac.New(sha256.New, []byte(secret))
	if _, err := mac.Write(body); err != nil {
		t.Fatal(err)
	}
	return hex.EncodeToString(mac.Sum(nil))
}

func issueRevision(t *testing.T, issueID string) int64 {
	t.Helper()
	var revision int64
	if err := testPool.QueryRow(context.Background(), `SELECT revision FROM issue WHERE id=$1`, issueID).Scan(&revision); err != nil {
		t.Fatal(err)
	}
	return revision
}

func linkedRemoteID(t *testing.T, issueID string) string {
	t.Helper()
	var remoteID string
	if err := testPool.QueryRow(context.Background(), `SELECT linear_issue_id FROM linear_issue_link WHERE patchbay_issue_id=$1`, issueID).Scan(&remoteID); err != nil {
		t.Fatalf("no remote mapping for issue %s: %v", issueID, err)
	}
	return remoteID
}

// ---------------------------------------------------------------------------
// Outbox: durability, ordering, acknowledgement
// ---------------------------------------------------------------------------

// The outbox has to be written by the same transaction as the issue itself,
// otherwise a crash between the two loses the change with nothing left to
// replay it from. That is why the enqueue lives in a trigger rather than in
// handler code, and why this test writes an issue and asserts the row appeared
// without ever calling the integration.
func TestLinearWorkerPublishesTriggerBackedOutbox(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "publish", api)
	issueID := dbfx.Issue(t, "Publish me", testutil.Cols{"project_id": f.projectID, "description": "body", "priority": "high"})

	var queued int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND issue_id=$2 AND event_type='issue_created' AND processed_at IS NULL`, f.bindingID, issueID).Scan(&queued); err != nil {
		t.Fatal(err)
	}
	if queued != 1 {
		t.Fatalf("queued outbox rows = %d, want 1", queued)
	}

	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("worker did not claim the queued outbox row")
	}
	if len(api.created) != 1 || api.created[0].Title != "Publish me" || api.created[0].ProjectID != "linear-project" || api.created[0].Priority != 2 {
		t.Fatalf("created = %+v", api.created)
	}

	var processed bool
	var lockedBy *string
	if err := testPool.QueryRow(context.Background(), `SELECT processed_at IS NOT NULL, locked_by FROM linear_sync_outbox WHERE binding_id=$1 AND issue_id=$2`, f.bindingID, issueID).Scan(&processed, &lockedBy); err != nil {
		t.Fatal(err)
	}
	if !processed || lockedBy != nil {
		t.Fatalf("acknowledgement left processed=%v locked_by=%v", processed, lockedBy)
	}

	var identifier string
	if err := testPool.QueryRow(context.Background(), `SELECT linear_identifier FROM linear_issue_link WHERE binding_id=$1 AND patchbay_issue_id=$2`, f.bindingID, issueID).Scan(&identifier); err != nil {
		t.Fatalf("publish did not record the remote mapping: %v", err)
	}
	if identifier != "ENG-1" {
		t.Fatalf("identifier = %q", identifier)
	}
}

// A second event for an issue whose first event is still in flight must not be
// claimable. Without that guard two workers hold `issue_created` and
// `issue_updated` at the same time, neither can see the link the other is about
// to write, and the issue is created twice in Linear.
func TestLinearWorkerKeepsOutboxFIFOPerIssue(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	issueID := dbfx.Issue(t, "First title", testutil.Cols{"project_id": f.projectID})
	if _, err := testPool.Exec(context.Background(), `UPDATE issue SET title='Second title', revision=revision+1 WHERE id=$1`, issueID); err != nil {
		t.Fatal(err)
	}

	var pending int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1 AND processed_at IS NULL`, issueID).Scan(&pending); err != nil {
		t.Fatal(err)
	}
	if pending != 2 {
		t.Fatalf("pending outbox rows = %d, want 2 (create then update)", pending)
	}

	claimed, ok, err := f.worker.claimOutbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("first claim ok=%v err=%v", ok, err)
	}
	if claimed.EventType != "issue_created" {
		t.Fatalf("first claim took %q, want the oldest event", claimed.EventType)
	}
	if _, second, err := f.worker.claimOutbox(context.Background()); err != nil || second {
		t.Fatalf("second claim took a row for an issue already in flight (ok=%v err=%v)", second, err)
	}

	if err := f.worker.handleOutbox(context.Background(), claimed); err != nil {
		t.Fatal(err)
	}
	f.worker.finish(context.Background(), "linear_sync_outbox", claimed.ID, pgtype.UUID{}, claimed.Attempts, claimed.MaxAttempts, nil)

	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("update event was not claimable once the create finished")
	}
	created, updated, _, _ := api.calls()
	if created != 1 || updated != 1 {
		t.Fatalf("create/update calls = %d/%d, want 1/1", created, updated)
	}
	if api.updated[0].Title != "Second title" {
		t.Fatalf("update sent %q", api.updated[0].Title)
	}
}

// A claim is a lease, not a delete: the row stays visible and re-enters the
// queue on its own if the worker holding it dies. Both halves of that matter,
// so both are asserted here.
func TestLinearWorkerClaimIsALeaseThatExpires(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "publish", api)
	dbfx.Issue(t, "Leased", testutil.Cols{"project_id": f.projectID})

	first, ok, err := f.worker.claimOutbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("claim ok=%v err=%v", ok, err)
	}
	if _, ok, err := f.worker.claimOutbox(context.Background()); err != nil || ok {
		t.Fatalf("leased row was claimed twice (ok=%v err=%v)", ok, err)
	}

	var lockedBy string
	var lockedUntil time.Time
	if err := testPool.QueryRow(context.Background(), `SELECT locked_by, locked_until FROM linear_sync_outbox WHERE id=$1`, first.ID).Scan(&lockedBy, &lockedUntil); err != nil {
		t.Fatal(err)
	}
	if lockedBy != f.worker.workerID || !lockedUntil.After(time.Now()) {
		t.Fatalf("lease locked_by=%q locked_until=%v", lockedBy, lockedUntil)
	}

	if _, err := testPool.Exec(context.Background(), `UPDATE linear_sync_outbox SET locked_until=now()-interval '1 second' WHERE id=$1`, first.ID); err != nil {
		t.Fatal(err)
	}
	retaken, ok, err := f.worker.claimOutbox(context.Background())
	if err != nil || !ok {
		t.Fatalf("expired lease was not reclaimed (ok=%v err=%v)", ok, err)
	}
	if retaken.ID != first.ID || retaken.Attempts != first.Attempts+1 {
		t.Fatalf("reclaim id=%v attempts=%d (first %d)", retaken.ID, retaken.Attempts, first.Attempts)
	}
}

// Retry and dead-letter are separate outcomes: a failure below the attempt
// limit has to come back with a delay, and only the last one may stop.
func TestLinearWorkerBacksOffThenDeadLetters(t *testing.T) {
	api := &fakeLinearAPI{err: errors.New("provider unavailable")}
	f := setupLinearWorker(t, "publish", api)
	issueID := dbfx.Issue(t, "Will fail", testutil.Cols{"project_id": f.projectID})
	if _, err := testPool.Exec(context.Background(), `UPDATE linear_sync_outbox SET max_attempts=2 WHERE issue_id=$1`, issueID); err != nil {
		t.Fatal(err)
	}

	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("worker did not claim the outbox row")
	}
	var attempts int
	var dead bool
	var lastError string
	var retryIn float64
	if err := testPool.QueryRow(context.Background(), `SELECT attempts, dead_lettered_at IS NOT NULL, last_error, extract(epoch FROM available_at-now()) FROM linear_sync_outbox WHERE issue_id=$1`, issueID).Scan(&attempts, &dead, &lastError, &retryIn); err != nil {
		t.Fatal(err)
	}
	if attempts != 1 || dead || lastError != "provider unavailable" || retryIn <= 0 {
		t.Fatalf("first failure attempts=%d dead=%v error=%q retry_in=%.1fs", attempts, dead, lastError, retryIn)
	}

	var connectionError string
	if err := testPool.QueryRow(context.Background(), `SELECT last_error FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&connectionError); err != nil {
		t.Fatalf("push failure was not attributed to the connection: %v", err)
	}
	if connectionError != "provider unavailable" {
		t.Fatalf("connection last_error = %q", connectionError)
	}

	if _, err := testPool.Exec(context.Background(), `UPDATE linear_sync_outbox SET available_at=now() WHERE issue_id=$1`, issueID); err != nil {
		t.Fatal(err)
	}
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("worker did not retry the outbox row")
	}
	if err := testPool.QueryRow(context.Background(), `SELECT attempts, dead_lettered_at IS NOT NULL FROM linear_sync_outbox WHERE issue_id=$1`, issueID).Scan(&attempts, &dead); err != nil {
		t.Fatal(err)
	}
	if attempts != 2 || !dead {
		t.Fatalf("final failure attempts=%d dead=%v, want 2/true", attempts, dead)
	}
	if _, ok, err := f.worker.claimOutbox(context.Background()); err != nil || ok {
		t.Fatalf("dead-lettered row is still claimable (ok=%v err=%v)", ok, err)
	}
}

// Deleting a published issue has to reach Linear, and the link has to be
// tombstoned rather than dropped so a later remote event for the same issue is
// recognised as belonging to a deleted mapping.
func TestLinearWorkerPublishesLocalDeletion(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "publish", api)
	issueID := dbfx.Issue(t, "Delete me", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("create was not published")
	}
	remoteID := linkedRemoteID(t, issueID)
	if _, err := testPool.Exec(context.Background(), `DELETE FROM issue WHERE id=$1`, issueID); err != nil {
		t.Fatal(err)
	}
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("delete was not published")
	}
	if len(api.deleted) != 1 || api.deleted[0] != remoteID {
		t.Fatalf("deleted = %v, want [%s]", api.deleted, remoteID)
	}
	var syncStatus string
	if err := testPool.QueryRow(context.Background(), `SELECT sync_status FROM linear_issue_link WHERE patchbay_issue_id=$1`, issueID).Scan(&syncStatus); err != nil {
		t.Fatal(err)
	}
	if syncStatus != "deleted" {
		t.Fatalf("link sync_status = %q", syncStatus)
	}
}

// Pausing a binding is how an operator stops publishing. Work queued before
// that moment must be acknowledged, not retried until it dead-letters and
// reports the integration as broken.
func TestLinearWorkerDropsOutboxForInactiveBinding(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "publish", api)
	dbfx.Issue(t, "Queued before pause", testutil.Cols{"project_id": f.projectID})
	if _, err := testPool.Exec(context.Background(), `UPDATE linear_project_binding SET status='paused' WHERE id=$1`, f.bindingID); err != nil {
		t.Fatal(err)
	}
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("worker did not claim the outbox row")
	}
	if created, _, _, _ := api.calls(); created != 0 {
		t.Fatalf("paused binding still published %d issues", created)
	}
	var processed, dead bool
	if err := testPool.QueryRow(context.Background(), `SELECT processed_at IS NOT NULL, dead_lettered_at IS NOT NULL FROM linear_sync_outbox WHERE binding_id=$1`, f.bindingID).Scan(&processed, &dead); err != nil {
		t.Fatal(err)
	}
	if !processed || dead {
		t.Fatalf("processed=%v dead=%v, want acknowledged", processed, dead)
	}
}

// ---------------------------------------------------------------------------
// Inbox: import, apply, loop suppression, conflicts
// ---------------------------------------------------------------------------

// Importing must produce real local issues, and — the property the whole
// integration rests on — must not re-enter the outbox. A remote apply that
// enqueued its own change would bounce straight back to Linear and never
// settle.
func TestLinearWorkerImportsWithoutOutboxEcho(t *testing.T) {
	remoteID := "20000000-0000-0000-0000-000000000002"
	api := &fakeLinearAPI{listed: []linear.Issue{{ID: remoteID, Identifier: "ENG-2", Title: "Remote issue", Description: "remote body", StateID: "remote-todo", ProjectID: "linear-project", TeamID: "linear-team", Priority: 3, UpdatedAt: time.Now()}}}
	f := setupLinearWorker(t, "two_way", api)
	payload, err := json.Marshal(map[string]string{"binding_id": f.bindingID})
	if err != nil {
		t.Fatal(err)
	}
	inboxID := dbfx.Insert(t, "linear_sync_inbox", testutil.Cols{
		"connection_id": f.connectionID,
		"delivery_id":   "initial-import:" + f.bindingID,
		"event_type":    "initial_import",
		"payload":       payload,
	})

	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("worker did not claim the import request")
	}

	var issueID, title, status, priority string
	if err := testPool.QueryRow(context.Background(), `SELECT id,title,status,priority FROM issue WHERE workspace_id=$1 AND origin_type='linear' AND origin_id=$2`, testWorkspaceID, remoteID).Scan(&issueID, &title, &status, &priority); err != nil {
		t.Fatalf("import did not create the local issue: %v", err)
	}
	if title != "Remote issue" || status != "todo" || priority != "medium" {
		t.Fatalf("imported issue title=%q status=%q priority=%q", title, status, priority)
	}

	var echoes int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1`, issueID).Scan(&echoes); err != nil {
		t.Fatal(err)
	}
	if echoes != 0 {
		t.Fatalf("remote apply enqueued %d outbound echoes", echoes)
	}

	var processed bool
	if err := testPool.QueryRow(context.Background(), `SELECT processed_at IS NOT NULL FROM linear_sync_inbox WHERE id=$1`, inboxID).Scan(&processed); err != nil {
		t.Fatal(err)
	}
	if !processed {
		t.Fatal("import request was not acknowledged")
	}
}

// Re-importing an unchanged project must be a complete no-op locally. This is
// what makes the poll fallback safe to run beside the webhooks.
func TestLinearWorkerReimportOfUnchangedIssueTouchesNothing(t *testing.T) {
	remoteID := "20000000-0000-0000-0000-000000000003"
	updatedAt := time.Now().Add(-time.Hour).UTC()
	api := &fakeLinearAPI{listed: []linear.Issue{{ID: remoteID, Identifier: "ENG-3", Title: "Stable", Description: "same", StateID: "remote-todo", ProjectID: "linear-project", TeamID: "linear-team", Priority: 3, UpdatedAt: updatedAt}}}
	f := setupLinearWorker(t, "two_way", api)
	payload, err := json.Marshal(map[string]string{"binding_id": f.bindingID})
	if err != nil {
		t.Fatal(err)
	}
	dbfx.Insert(t, "linear_sync_inbox", testutil.Cols{"connection_id": f.connectionID, "delivery_id": "import-1", "event_type": "initial_import", "payload": payload})
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("first import was not processed")
	}
	var issueID string
	if err := testPool.QueryRow(context.Background(), `SELECT id FROM issue WHERE origin_type='linear' AND origin_id=$1`, remoteID).Scan(&issueID); err != nil {
		t.Fatal(err)
	}
	before := issueRevision(t, issueID)

	dbfx.Insert(t, "linear_sync_inbox", testutil.Cols{"connection_id": f.connectionID, "delivery_id": "import-2", "event_type": "binding_poll", "payload": payload})
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("poll was not processed")
	}
	if after := issueRevision(t, issueID); after != before {
		t.Fatalf("unchanged re-import bumped revision %d -> %d", before, after)
	}
	var echoes int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1`, issueID).Scan(&echoes); err != nil {
		t.Fatal(err)
	}
	if echoes != 0 {
		t.Fatalf("unchanged re-import enqueued %d outbound events", echoes)
	}
	api.mu.Lock()
	commentLists := api.commentLists
	api.mu.Unlock()
	if commentLists != 1 {
		t.Fatalf("comment history lists=%d, want one initial import and none during poll", commentLists)
	}
}

// The echo of our own push carries exactly what we sent. It has to land as
// "nothing changed", not as a remote edit, or every publish would ping-pong.
func TestLinearWorkerSuppressesEchoOfItsOwnPush(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	issueID := dbfx.Issue(t, "Pushed title", testutil.Cols{"project_id": f.projectID, "description": "pushed body", "priority": "high"})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("issue was not published")
	}
	remoteID := linkedRemoteID(t, issueID)
	before := issueRevision(t, issueID)

	f.queueWebhook(t, "echo-1", webhookPayload(t, "update", time.Now().UnixMilli(), map[string]any{
		"id": remoteID, "identifier": "ENG-1", "title": "Pushed title", "description": "pushed body",
		"priority": 2, "updatedAt": time.Now().UTC().Format(time.RFC3339Nano),
		"project": map[string]any{"id": "linear-project"}, "team": map[string]any{"id": "linear-team"},
		"state": map[string]any{"id": "remote-todo", "type": "unstarted"},
	}))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("echo was not processed")
	}

	if after := issueRevision(t, issueID); after != before {
		t.Fatalf("echo of our own push bumped revision %d -> %d", before, after)
	}
	var conflicts int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_conflict WHERE patchbay_issue_id=$1`, issueID).Scan(&conflicts); err != nil {
		t.Fatal(err)
	}
	if conflicts != 0 {
		t.Fatalf("echo recorded %d conflicts", conflicts)
	}
	var outbound int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE issue_id=$1 AND processed_at IS NULL`, issueID).Scan(&outbound); err != nil {
		t.Fatal(err)
	}
	if outbound != 0 {
		t.Fatalf("echo re-queued %d outbound events", outbound)
	}
}

// A remote-only edit applies cleanly and moves the link's baseline forward.
func TestLinearWorkerAppliesRemoteOnlyEdit(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	issueID := dbfx.Issue(t, "Pushed title", testutil.Cols{"project_id": f.projectID, "description": "pushed body"})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("issue was not published")
	}
	remoteID := linkedRemoteID(t, issueID)

	f.queueWebhook(t, "remote-edit", webhookPayload(t, "update", time.Now().UnixMilli(), map[string]any{
		"id": remoteID, "identifier": "ENG-1", "title": "Renamed in Linear", "description": "pushed body",
		"priority": 0, "updatedAt": time.Now().UTC().Format(time.RFC3339Nano),
		"project": map[string]any{"id": "linear-project"}, "team": map[string]any{"id": "linear-team"},
		"state": map[string]any{"id": "remote-doing", "type": "started"},
	}))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("remote edit was not processed")
	}

	var title, status string
	if err := testPool.QueryRow(context.Background(), `SELECT title,status FROM issue WHERE id=$1`, issueID).Scan(&title, &status); err != nil {
		t.Fatal(err)
	}
	if title != "Renamed in Linear" || status != "in_progress" {
		t.Fatalf("title=%q status=%q", title, status)
	}
	var syncStatus, snapshotTitle string
	if err := testPool.QueryRow(context.Background(), `SELECT sync_status, last_common_snapshot->>'title' FROM linear_issue_link WHERE patchbay_issue_id=$1`, issueID).Scan(&syncStatus, &snapshotTitle); err != nil {
		t.Fatal(err)
	}
	if syncStatus != "active" || snapshotTitle != "Renamed in Linear" {
		t.Fatalf("link sync_status=%q snapshot title=%q", syncStatus, snapshotTitle)
	}
}

// When both sides moved the same field away from the shared baseline the
// remote value must not silently win: the local value is kept, the divergence
// is recorded for a human, and the link is flagged.
func TestLinearWorkerRecordsConflictWhenBothSidesMoved(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	issueID := dbfx.Issue(t, "Shared title", testutil.Cols{"project_id": f.projectID, "description": "body"})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("issue was not published")
	}
	remoteID := linkedRemoteID(t, issueID)
	if _, err := testPool.Exec(context.Background(), `UPDATE issue SET title='Local title' WHERE id=$1`, issueID); err != nil {
		t.Fatal(err)
	}

	remoteEvent := func(at int64) []byte {
		return webhookPayload(t, "update", at, map[string]any{
			"id": remoteID, "identifier": "ENG-1", "title": "Remote title", "description": "body",
			"priority": 0, "updatedAt": time.Now().UTC().Format(time.RFC3339Nano),
			"project": map[string]any{"id": "linear-project"}, "team": map[string]any{"id": "linear-team"},
			"state": map[string]any{"id": "remote-todo", "type": "unstarted"},
		})
	}
	now := time.Now().UnixMilli()
	f.queueWebhook(t, "conflict-1", remoteEvent(now))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("conflicting event was not processed")
	}

	var title string
	if err := testPool.QueryRow(context.Background(), `SELECT title FROM issue WHERE id=$1`, issueID).Scan(&title); err != nil {
		t.Fatal(err)
	}
	if title != "Local title" {
		t.Fatalf("conflict overwrote the local value: title=%q", title)
	}
	var field, local, remote, base, status string
	if err := testPool.QueryRow(context.Background(), `SELECT field, local_value #>> '{}', remote_value #>> '{}', base_value #>> '{}', status FROM linear_sync_conflict WHERE patchbay_issue_id=$1`, issueID).Scan(&field, &local, &remote, &base, &status); err != nil {
		t.Fatalf("no conflict recorded: %v", err)
	}
	if field != "title" || local != "Local title" || remote != "Remote title" || base != "Shared title" || status != "open" {
		t.Fatalf("conflict field=%q local=%q remote=%q base=%q status=%q", field, local, remote, base, status)
	}
	var syncStatus string
	if err := testPool.QueryRow(context.Background(), `SELECT sync_status FROM linear_issue_link WHERE patchbay_issue_id=$1`, issueID).Scan(&syncStatus); err != nil {
		t.Fatal(err)
	}
	if syncStatus != "conflict" {
		t.Fatalf("link sync_status = %q", syncStatus)
	}

	// A later delivery of the same divergence must reuse the open conflict
	// row rather than failing on the partial unique index or stacking
	// duplicates for the same field.
	f.queueWebhook(t, "conflict-2", remoteEvent(now+1))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("second conflicting event was not processed")
	}
	var conflicts int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_conflict WHERE patchbay_issue_id=$1 AND status='open'`, issueID).Scan(&conflicts); err != nil {
		t.Fatal(err)
	}
	if conflicts != 1 {
		t.Fatalf("open conflicts after redelivery = %d, want 1", conflicts)
	}
}

// Webhook feeds redeliver and reorder. An event older than what the link has
// already absorbed must not be replayed over newer state.
func TestLinearWorkerIgnoresStaleRemoteEvent(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	issueID := dbfx.Issue(t, "Original", testutil.Cols{"project_id": f.projectID, "description": "body"})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("issue was not published")
	}
	remoteID := linkedRemoteID(t, issueID)
	newer := time.Now().UnixMilli()
	body := func(title string) map[string]any {
		return map[string]any{
			"id": remoteID, "identifier": "ENG-1", "title": title, "description": "body",
			"priority": 0, "updatedAt": time.Now().UTC().Format(time.RFC3339Nano),
			"project": map[string]any{"id": "linear-project"}, "team": map[string]any{"id": "linear-team"},
			"state": map[string]any{"id": "remote-todo", "type": "unstarted"},
		}
	}
	f.queueWebhook(t, "stale-newer", webhookPayload(t, "update", newer, body("Newest remote title")))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("newer event was not processed")
	}
	f.queueWebhook(t, "stale-older", webhookPayload(t, "update", newer-60_000, body("Superseded remote title")))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("older event was not claimed")
	}

	var title string
	if err := testPool.QueryRow(context.Background(), `SELECT title FROM issue WHERE id=$1`, issueID).Scan(&title); err != nil {
		t.Fatal(err)
	}
	if title != "Newest remote title" {
		t.Fatalf("stale event was replayed over newer state: title=%q", title)
	}
}

// A remote delete has to land locally as a cancellation with a tombstoned
// link; dropping the row would let the next import recreate the issue.
func TestLinearWorkerAppliesRemoteDeletion(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	issueID := dbfx.Issue(t, "Doomed", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("issue was not published")
	}
	remoteID := linkedRemoteID(t, issueID)
	f.queueWebhook(t, "remote-delete", webhookPayload(t, "remove", time.Now().UnixMilli(), map[string]any{
		"id": remoteID, "identifier": "ENG-1", "title": "Doomed",
		"project": map[string]any{"id": "linear-project"}, "team": map[string]any{"id": "linear-team"},
	}))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("remote delete was not processed")
	}
	var status, syncStatus string
	if err := testPool.QueryRow(context.Background(), `SELECT i.status, l.sync_status FROM issue i JOIN linear_issue_link l ON l.patchbay_issue_id=i.id WHERE i.id=$1`, issueID).Scan(&status, &syncStatus); err != nil {
		t.Fatal(err)
	}
	if status != "cancelled" || syncStatus != "deleted" {
		t.Fatalf("issue status=%q link sync_status=%q", status, syncStatus)
	}
}

// An event for a Linear project nobody bound must be acknowledged and dropped.
// Retrying it forever would eventually dead-letter and report a healthy
// integration as broken.
func TestLinearWorkerAcknowledgesEventForUnboundProject(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	f.queueWebhook(t, "unbound", webhookPayload(t, "update", time.Now().UnixMilli(), map[string]any{
		"id": "30000000-0000-0000-0000-000000000009", "identifier": "OTH-1", "title": "Someone else's issue",
		"project": map[string]any{"id": "other-project"}, "team": map[string]any{"id": "other-team"},
		"state": map[string]any{"id": "x", "type": "unstarted"},
	}))
	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("event was not claimed")
	}
	var processed, dead bool
	if err := testPool.QueryRow(context.Background(), `SELECT processed_at IS NOT NULL, dead_lettered_at IS NOT NULL FROM linear_sync_inbox WHERE connection_id=$1 AND delivery_id='unbound'`, f.connectionID).Scan(&processed, &dead); err != nil {
		t.Fatal(err)
	}
	if !processed || dead {
		t.Fatalf("processed=%v dead=%v", processed, dead)
	}
	var issues int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM issue WHERE workspace_id=$1 AND origin_type='linear'`, testWorkspaceID).Scan(&issues); err != nil {
		t.Fatal(err)
	}
	if issues != 0 {
		t.Fatalf("unbound event created %d local issues", issues)
	}
}

// ---------------------------------------------------------------------------
// Polling fallback, tokens, and feature flags
// ---------------------------------------------------------------------------

// Polling is the fallback for a webhook that never arrived. It goes through
// the same inbox as the webhooks, and its time-bucketed delivery id is what
// stops N replicas from enqueuing N polls per binding.
func TestLinearWorkerPollFallbackEnqueuesOncePerBucket(t *testing.T) {
	remoteID := "20000000-0000-0000-0000-00000000000a"
	api := &fakeLinearAPI{listed: []linear.Issue{{ID: remoteID, Identifier: "ENG-9", Title: "Missed webhook", StateID: "remote-todo", ProjectID: "linear-project", TeamID: "linear-team", UpdatedAt: time.Now()}}}
	f := setupLinearWorker(t, "two_way", api)

	for i := 0; i < 3; i++ {
		if err := f.worker.enqueuePolls(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	var polls int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND event_type='binding_poll'`, f.connectionID).Scan(&polls); err != nil {
		t.Fatal(err)
	}
	if polls != 1 {
		t.Fatalf("three enqueues in one bucket produced %d poll rows, want 1", polls)
	}

	if !f.worker.processOneInbox(context.Background()) {
		t.Fatal("poll was not processed")
	}
	var title string
	if err := testPool.QueryRow(context.Background(), `SELECT title FROM issue WHERE origin_type='linear' AND origin_id=$1`, remoteID).Scan(&title); err != nil {
		t.Fatalf("poll did not import the missed issue: %v", err)
	}
	if title != "Missed webhook" {
		t.Fatalf("title = %q", title)
	}

	// A different bucket width is a different delivery id, so the next
	// window polls again rather than being deduplicated forever.
	f.worker.pollInterval = time.Second
	if err := f.worker.enqueuePolls(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND event_type='binding_poll'`, f.connectionID).Scan(&polls); err != nil {
		t.Fatal(err)
	}
	if polls != 2 {
		t.Fatalf("new bucket produced %d poll rows total, want 2", polls)
	}
}

// A publish-only binding has nothing to pull, so it must not be polled.
func TestLinearWorkerDoesNotPollPublishOnlyBinding(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "publish", api)
	if err := f.worker.enqueuePolls(context.Background()); err != nil {
		t.Fatal(err)
	}
	var polls int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND event_type='binding_poll'`, f.connectionID).Scan(&polls); err != nil {
		t.Fatal(err)
	}
	if polls != 0 {
		t.Fatalf("publish-only binding enqueued %d polls", polls)
	}
}

// An access token that is about to expire is refreshed and re-sealed in place
// so the next claim finds a usable credential.
func TestLinearWorkerRefreshesExpiringToken(t *testing.T) {
	api := &fakeLinearAPI{refresh: linear.Token{AccessToken: "fresh-access", RefreshToken: "fresh-refresh", Scope: "read write", ExpiresIn: time.Hour}}
	f := setupLinearWorker(t, "publish", api)
	if _, err := testPool.Exec(context.Background(), `UPDATE linear_connection SET token_expires_at=now()+interval '30 seconds' WHERE id=$1`, f.connectionID); err != nil {
		t.Fatal(err)
	}
	dbfx.Issue(t, "Needs a fresh token", testutil.Cols{"project_id": f.projectID})

	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("worker did not claim the outbox row")
	}
	if _, _, _, refreshed := api.calls(); refreshed != 1 {
		t.Fatalf("refresh calls = %d, want 1", refreshed)
	}
	var sealed []byte
	var expires time.Time
	if err := testPool.QueryRow(context.Background(), `SELECT access_token_encrypted, token_expires_at FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&sealed, &expires); err != nil {
		t.Fatal(err)
	}
	plain, err := f.box.Open(sealed)
	if err != nil {
		t.Fatal(err)
	}
	if string(plain) != "fresh-access" {
		t.Fatalf("stored access token = %q", plain)
	}
	if !expires.After(time.Now().Add(50 * time.Minute)) {
		t.Fatalf("token_expires_at was not advanced: %v", expires)
	}
}

// A refresh the provider rejects is not a transient failure: the connection
// needs a human to reauthorize, and it has to say so rather than publishing
// with a credential that will never work again.
func TestLinearWorkerMarksConnectionForReauthorizationOnRefreshFailure(t *testing.T) {
	api := &fakeLinearAPI{authErr: errors.New("invalid_grant")}
	f := setupLinearWorker(t, "publish", api)
	if _, err := testPool.Exec(context.Background(), `UPDATE linear_connection SET token_expires_at=now()-interval '1 minute' WHERE id=$1`, f.connectionID); err != nil {
		t.Fatal(err)
	}
	dbfx.Issue(t, "Cannot publish", testutil.Cols{"project_id": f.projectID})

	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("worker did not claim the outbox row")
	}
	var status string
	if err := testPool.QueryRow(context.Background(), `SELECT status FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&status); err != nil {
		t.Fatal(err)
	}
	if status != "reauthorization_required" {
		t.Fatalf("connection status = %q", status)
	}
	if created, _, _, _ := api.calls(); created != 0 {
		t.Fatalf("worker published %d issues with an unusable token", created)
	}
}

// Pull and push are independently deployable switches. With both off the
// worker must leave queued work alone rather than draining it silently.
func TestLinearWorkerRespectsDirectionFlags(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	payload, err := json.Marshal(map[string]string{"binding_id": f.bindingID})
	if err != nil {
		t.Fatal(err)
	}
	dbfx.Insert(t, "linear_sync_inbox", testutil.Cols{"connection_id": f.connectionID, "delivery_id": "flagged", "event_type": "initial_import", "payload": payload})
	dbfx.Issue(t, "Not published while off", testutil.Cols{"project_id": f.projectID})

	off := NewLinearWorker(testPool, testPool, f.box, api, "client", "secret", false, false)
	off.interval = 5 * time.Millisecond
	off.pollInterval = 5 * time.Millisecond
	offCtx, cancelOff := context.WithTimeout(context.Background(), 150*time.Millisecond)
	off.Run(offCtx)
	cancelOff()

	var pendingInbox, pendingOutbox, polls int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND processed_at IS NULL AND event_type='initial_import'`, f.connectionID).Scan(&pendingInbox); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND processed_at IS NULL`, f.bindingID).Scan(&pendingOutbox); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND event_type='binding_poll'`, f.connectionID).Scan(&polls); err != nil {
		t.Fatal(err)
	}
	if pendingInbox != 1 || pendingOutbox != 1 || polls != 0 {
		t.Fatalf("with both directions off: inbox=%d outbox=%d polls=%d, want 1/1/0", pendingInbox, pendingOutbox, polls)
	}
	if created, updated, deleted, _ := api.calls(); created+updated+deleted != 0 {
		t.Fatal("disabled worker still called the provider")
	}

	on := NewLinearWorker(testPool, testPool, f.box, api, "client", "secret", true, true)
	on.interval = 5 * time.Millisecond
	on.pollInterval = time.Hour
	onCtx, cancelOn := context.WithTimeout(context.Background(), time.Second)
	on.Run(onCtx)
	cancelOn()
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_outbox WHERE binding_id=$1 AND processed_at IS NULL`, f.bindingID).Scan(&pendingOutbox); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND processed_at IS NULL`, f.connectionID).Scan(&pendingInbox); err != nil {
		t.Fatal(err)
	}
	if pendingOutbox != 0 || pendingInbox != 0 {
		t.Fatalf("enabled worker left outbox=%d inbox=%d unprocessed", pendingOutbox, pendingInbox)
	}
}

// ---------------------------------------------------------------------------
// Webhook intake and revocation
// ---------------------------------------------------------------------------

func linearIntegrationHandler(t *testing.T, f linearFixture) *Handler {
	t.Helper()
	return &Handler{
		DB:                  testPool,
		TxStarter:           testPool,
		FeatureFlags:        linearTestFlags(true),
		LinearSecretBox:     f.box,
		LinearClientID:      "client",
		LinearClientSecret:  "secret",
		LinearWebhookSecret: "webhook-secret",
		LinearWorker:        f.worker,
	}
}

// The webhook endpoint is the wake path: it persists the delivery, dedupes
// redeliveries on the provider's own delivery id, and nudges the worker so the
// event is applied now instead of at the next poll.
func TestHandleLinearWebhookPersistsDedupesAndWakesWorker(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	h := linearIntegrationHandler(t, f)
	var organizationID string
	if err := testPool.QueryRow(context.Background(), `SELECT organization_id FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&organizationID); err != nil {
		t.Fatal(err)
	}
	timestamp := time.Now().UnixMilli()
	body, err := json.Marshal(map[string]any{
		"type": "Issue", "action": "update", "organizationId": organizationID,
		"webhookId":        "linear-hook-1",
		"webhookTimestamp": timestamp,
		"data":             map[string]any{"id": "40000000-0000-0000-0000-000000000004"},
	})
	if err != nil {
		t.Fatal(err)
	}
	send := func(delivery string) *httptest.ResponseRecorder {
		request := httptest.NewRequest(http.MethodPost, "/api/webhooks/linear", strings.NewReader(string(body)))
		request.Header.Set("Linear-Signature", linearTestSignature(t, "webhook-secret", body))
		request.Header.Set("Linear-Timestamp", fmt.Sprint(timestamp))
		request.Header.Set("Linear-Delivery", delivery)
		recorder := httptest.NewRecorder()
		h.HandleLinearWebhook(recorder, request)
		return recorder
	}

	// Drain the startup nudge so the assertion below sees only what this
	// request produced.
	select {
	case <-f.worker.wake:
	default:
	}

	first := send("delivery-1")
	if first.Code != http.StatusOK {
		t.Fatalf("status = %d body = %s", first.Code, first.Body.String())
	}
	select {
	case <-f.worker.wake:
	default:
		t.Fatal("accepted webhook did not wake the worker")
	}

	replay := send("delivery-1")
	if replay.Code != http.StatusOK || !strings.Contains(replay.Body.String(), `"duplicate":true`) {
		t.Fatalf("redelivery status = %d body = %s", replay.Code, replay.Body.String())
	}
	var stored int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_sync_inbox WHERE connection_id=$1 AND delivery_id='delivery-1'`, f.connectionID).Scan(&stored); err != nil {
		t.Fatal(err)
	}
	if stored != 1 {
		t.Fatalf("redelivery stored %d rows", stored)
	}
}

// Disconnecting revokes the provider credential and tombstones only the
// connection. Binding/link history remains auditable and can be reactivated by
// a later OAuth install; workspace deletion owns the destructive cleanup.
func TestDisconnectLinearRevokesAndMarksConnection(t *testing.T) {
	api := &fakeLinearAPI{}
	f := setupLinearWorker(t, "two_way", api)
	h := linearIntegrationHandler(t, f)
	issueID := dbfx.Issue(t, "Linked", testutil.Cols{"project_id": f.projectID})
	if !f.worker.processOneOutbox(context.Background()) {
		t.Fatal("issue was not published")
	}
	dbfx.Insert(t, "linear_member_binding", testutil.Cols{"workspace_id": testWorkspaceID, "connection_id": f.connectionID, "patchbay_user_id": testUserID, "linear_user_id": "linear-user"})

	request := httptest.NewRequest(http.MethodDelete, "/api/workspaces/"+testWorkspaceID+"/linear/connection", nil)
	routeCtx := chi.NewRouteContext()
	routeCtx.URLParams.Add("id", testWorkspaceID)
	request = request.WithContext(context.WithValue(request.Context(), chi.RouteCtxKey, routeCtx))
	recorder := httptest.NewRecorder()
	h.DisconnectLinear(recorder, request)
	if recorder.Code != http.StatusNoContent {
		t.Fatalf("status = %d body = %s", recorder.Code, recorder.Body.String())
	}
	if len(api.revoked) != 1 || api.revoked[0] != "access" {
		t.Fatalf("revoked = %v, want the stored access token", api.revoked)
	}

	var status string
	if err := testPool.QueryRow(context.Background(), `SELECT status FROM linear_connection WHERE id=$1`, f.connectionID).Scan(&status); err != nil {
		t.Fatal(err)
	}
	if status != "revoked" {
		t.Fatalf("connection status=%q, want revoked", status)
	}
	var bindings, links int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_project_binding WHERE workspace_id=$1`, testWorkspaceID).Scan(&bindings); err != nil {
		t.Fatal(err)
	}
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM linear_issue_link WHERE workspace_id=$1`, testWorkspaceID).Scan(&links); err != nil {
		t.Fatal(err)
	}
	if bindings != 1 || links != 1 {
		t.Fatalf("disconnect removed audit state: bindings=%d links=%d", bindings, links)
	}
	var issues int
	if err := testPool.QueryRow(context.Background(), `SELECT count(*) FROM issue WHERE id=$1`, issueID).Scan(&issues); err != nil {
		t.Fatal(err)
	}
	if issues != 1 {
		t.Fatal("disconnect deleted the local issue")
	}
}

func TestLinearRetryDelayGrowsAndIsCapped(t *testing.T) {
	for _, tc := range []struct {
		attempt int32
		want    time.Duration
	}{
		{1, time.Second},
		{2, 2 * time.Second},
		{5, 16 * time.Second},
		{20, 900 * time.Second},
	} {
		if got := retryDelay(tc.attempt); got != tc.want {
			t.Errorf("retryDelay(%d) = %v, want %v", tc.attempt, got, tc.want)
		}
	}
}
