package handler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/ghsnapshot"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

const (
	workProductDiscoveryInterval = 30 * time.Second
	workProductDiscoveryBatch    = int32(100)
	workProductDiscoveryLease    = "5 minutes"
)

// WorkProductDiscoveryRuntime drains the durable execution-provenance queue.
// The queue is intentionally database-backed: a server restart, a GitHub rate
// limit, or a daemon disconnect must not lose the association attempt.
type WorkProductDiscoveryRuntime struct {
	handler   *Handler
	interval  time.Duration
	batchSize int32

	mu      sync.Mutex
	started bool
}

func NewWorkProductDiscoveryRuntime(h *Handler) *WorkProductDiscoveryRuntime {
	return &WorkProductDiscoveryRuntime{
		handler:   h,
		interval:  workProductDiscoveryInterval,
		batchSize: workProductDiscoveryBatch,
	}
}

// Start launches one bounded drain loop. It remains useful when GitHub is not
// configured: those rows converge to an explicit ineligible result instead of
// silently remaining pending forever.
func (r *WorkProductDiscoveryRuntime) Start(ctx context.Context) {
	if r == nil || r.handler == nil {
		return
	}
	r.mu.Lock()
	if r.started {
		r.mu.Unlock()
		return
	}
	r.started = true
	r.mu.Unlock()

	go func() {
		r.DrainOnce(ctx)
		ticker := time.NewTicker(r.interval)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				r.DrainOnce(ctx)
			}
		}
	}()
}

// DrainOnce is exported for focused acceptance tests and for terminal paths
// that want to kick the queue without waiting for the ticker.
func (r *WorkProductDiscoveryRuntime) DrainOnce(ctx context.Context) {
	if r == nil || r.handler == nil || r.handler.Queries == nil || r.handler.DB == nil {
		return
	}
	rows, err := r.handler.Queries.ListPendingExecutionDiscoveryTasks(ctx, r.batchSize)
	if err != nil {
		slog.Warn("work product discovery: list pending tasks failed", "error", err)
		return
	}
	for _, row := range rows {
		task, taskErr := r.handler.Queries.GetAgentTaskInWorkspace(ctx, db.GetAgentTaskInWorkspaceParams{
			ID:          row.TaskID,
			WorkspaceID: row.WorkspaceID,
		})
		if taskErr != nil {
			if isNotFound(taskErr) {
				if err := r.markMissingTask(ctx, row.WorkspaceID, row.TaskID); err != nil {
					slog.Warn("work product discovery: mark missing task failed", "task_id", uuidToString(row.TaskID), "error", err)
				}
				continue
			}
			slog.Warn("work product discovery: load task failed", "task_id", uuidToString(row.TaskID), "error", taskErr)
			continue
		}
		if !terminalTaskStatus(task.Status) {
			// Initial provenance is recorded while a task is running. It is not
			// eligible for discovery until the terminal callback marks it done.
			continue
		}
		if err := r.discoverTask(ctx, task, row.WorkspaceID); err != nil {
			slog.Warn("work product discovery: task drain failed", "task_id", uuidToString(task.ID), "error", err)
		}
	}
}

func terminalTaskStatus(status string) bool {
	switch status {
	case "completed", "failed", "cancelled":
		return true
	default:
		return false
	}
}

func (h *Handler) scheduleWorkProductDiscovery(ctx context.Context, task db.AgentTaskQueue, workspaceID pgtype.UUID, facts workProductExecutionFacts) error {
	if h.WorkProductDiscovery == nil {
		return nil
	}
	if err := h.WorkProductDiscovery.Schedule(ctx, task, workspaceID, facts); err != nil {
		slog.Warn("work product discovery: schedule failed", "task_id", uuidToString(task.ID), "error", err)
		return err
	}
	return nil
}

type workProductExecutionFacts struct {
	repoIdentity       string
	executionWorkspace string
	headBranch         string
	headSHA            string
	headState          string
}

func workProductExecutionFactsFromCompleteRequest(req TaskCompleteRequest) workProductExecutionFacts {
	return workProductExecutionFacts{
		repoIdentity:       req.ExecutionRepoIdentity,
		executionWorkspace: req.ExecutionWorkspace,
		headBranch:         req.ExecutionHeadBranch,
		headSHA:            req.ExecutionHeadSHA,
		headState:          req.ExecutionHeadState,
	}
}

func workProductExecutionFactsFromFailRequest(req TaskFailRequest) workProductExecutionFacts {
	return workProductExecutionFacts{
		repoIdentity:       req.ExecutionRepoIdentity,
		executionWorkspace: req.ExecutionWorkspace,
		headBranch:         req.ExecutionHeadBranch,
		headSHA:            req.ExecutionHeadSHA,
		headState:          req.ExecutionHeadState,
	}
}

func workProductExecutionFactsFromCancelAckRequest(req TaskCancelAckRequest) workProductExecutionFacts {
	return workProductExecutionFacts{
		repoIdentity:       req.ExecutionRepoIdentity,
		executionWorkspace: req.ExecutionWorkspace,
		headBranch:         req.ExecutionHeadBranch,
		headSHA:            req.ExecutionHeadSHA,
		headState:          req.ExecutionHeadState,
	}
}

func (f workProductExecutionFacts) empty() bool {
	return strings.TrimSpace(f.repoIdentity) == "" &&
		strings.TrimSpace(f.executionWorkspace) == "" &&
		strings.TrimSpace(f.headBranch) == "" &&
		strings.TrimSpace(f.headSHA) == "" &&
		strings.TrimSpace(f.headState) == ""
}

func normalizeWorkProductExecutionFacts(facts workProductExecutionFacts, task db.AgentTaskQueue) (workProductProvenanceValues, error) {
	validationTask := task
	// A daemon can report the checkout immediately after StartTask, before its
	// first session pin has populated task.work_dir. The daemon credential is
	// already scoped to this task/workspace; use that first report as the path
	// anchor, then enforce the stored task path on terminal discovery.
	if strings.TrimSpace(textWorkProductValue(validationTask.WorkDir)) == "" &&
		strings.TrimSpace(textWorkProductValue(validationTask.DurableWorkDir)) == "" &&
		strings.TrimSpace(facts.executionWorkspace) != "" {
		validationTask.WorkDir = pgtype.Text{String: strings.TrimSpace(facts.executionWorkspace), Valid: true}
	}
	return normalizeWorkProductProvenance(workProductProvenanceRequest{
		RepoIdentity:       facts.repoIdentity,
		ExecutionWorkspace: facts.executionWorkspace,
		HeadBranch:         facts.headBranch,
		HeadSHA:            facts.headSHA,
		HeadState:          facts.headState,
	}, validationTask)
}

// Schedule writes the terminal handoff and then kicks a bounded asynchronous
// drain. Existing facts are retained when an older daemon omits the new
// terminal fields; this is what makes the endpoint a rolling-compatible
// enhancement without inventing a second provenance row.
func (r *WorkProductDiscoveryRuntime) Schedule(ctx context.Context, task db.AgentTaskQueue, workspaceID pgtype.UUID, facts workProductExecutionFacts) error {
	if r == nil || r.handler == nil || r.handler.DB == nil || r.handler.Queries == nil || r.handler.TxStarter == nil {
		return errors.New("database unavailable")
	}
	tx, err := r.handler.TxStarter.Begin(ctx)
	if err != nil {
		return err
	}
	rollback := func() { _ = tx.Rollback(ctx) }
	if err := r.prepareTerminalHandoff(ctx, tx, r.handler.Queries.WithTx(tx), task, workspaceID, facts); err != nil {
		rollback()
		return err
	}
	if err := tx.Commit(ctx); err != nil {
		rollback()
		return err
	}
	r.kick(ctx, task, workspaceID)
	return nil
}

// prepareTerminalHandoff writes the complete durable discovery obligation to
// an existing transaction. Complete/fail handlers call this from the task
// status transaction so the two states cannot be separated by a process crash;
// Schedule uses the same helper for cancellation acknowledgements and the
// standalone provenance endpoint.
func (r *WorkProductDiscoveryRuntime) prepareTerminalHandoff(ctx context.Context, executor dbExecutor, queries *db.Queries, task db.AgentTaskQueue, workspaceID pgtype.UUID, facts workProductExecutionFacts) error {
	if r == nil || r.handler == nil || executor == nil || queries == nil {
		return errors.New("database unavailable")
	}
	values, err := normalizeWorkProductExecutionFacts(facts, task)
	if err != nil {
		// A malformed terminal evidence payload must not strand the already
		// completed task in a retry loop. Preserve a durable unknown checkout;
		// discovery will converge it to an explicit ineligible result, while the
		// task callback remains idempotently deliverable.
		slog.Warn("work product discovery: terminal facts invalid; retaining unknown provenance", "task_id", uuidToString(task.ID), "error", err)
		values, err = normalizeWorkProductExecutionFacts(workProductExecutionFacts{}, task)
		if err != nil {
			return fmt.Errorf("normalize fallback terminal execution facts: %w", err)
		}
	}

	existing, err := queries.ListExecutionProvenanceByTask(ctx, db.ListExecutionProvenanceByTaskParams{
		WorkspaceID: workspaceID,
		TaskID:      task.ID,
	})
	if err != nil {
		return err
	}
	if len(existing) == 0 || !facts.empty() {
		if _, err := upsertWorkProductExecutionProvenance(ctx, executor, task, workspaceID, values, true); err != nil {
			return err
		}
	}
	if skipped, err := markExplicitWorkProductRelation(ctx, executor, workspaceID, task.ID); err != nil {
		return err
	} else if !skipped {
		if err := markPendingWorkProductDiscovery(ctx, executor, workspaceID, task.ID); err != nil {
			return err
		}
	}
	return nil
}

func (r *WorkProductDiscoveryRuntime) kick(ctx context.Context, task db.AgentTaskQueue, workspaceID pgtype.UUID) {
	if r == nil {
		return
	}
	workerCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 2*time.Minute)
	go func() {
		defer cancel()
		if err := r.discoverTask(workerCtx, task, workspaceID); err != nil {
			slog.Warn("work product discovery: scheduled drain failed", "task_id", uuidToString(task.ID), "error", err)
		}
	}()
}

// daemonExecutionProvenanceRequest is intentionally separate from the
// task-token Work Product API request. DaemonAuth supplies the machine-level
// workspace boundary; the endpoint is used before the task has a durable
// work_dir pin and therefore cannot require X-Actor-Source=task_token.
type daemonExecutionProvenanceRequest struct {
	ExecutionRepoIdentity string `json:"execution_repo_identity"`
	ExecutionWorkspace    string `json:"execution_workspace"`
	ExecutionHeadBranch   string `json:"execution_head_branch"`
	ExecutionHeadSHA      string `json:"execution_head_sha"`
	ExecutionHeadState    string `json:"execution_head_state"`
	Finished              bool   `json:"finished"`
}

// RecordTaskExecutionProvenance receives an in-flight or terminal checkout
// snapshot from the daemon. It is authenticated by the existing daemon route
// and retains the same task/workspace binding as all other daemon task APIs.
func (h *Handler) RecordTaskExecutionProvenance(w http.ResponseWriter, r *http.Request) {
	taskID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "taskId"), "task id")
	if !ok {
		return
	}
	task, workspaceID, ok := h.requireDaemonTaskAccessWithWorkspace(w, r, uuidToString(taskID))
	if !ok {
		return
	}
	var request daemonExecutionProvenanceRequest
	if err := decodeWorkProductJSON(r, &request); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	request.ExecutionRepoIdentity = util.SanitizeTextForPostgres(request.ExecutionRepoIdentity)
	request.ExecutionWorkspace = util.SanitizeTextForPostgres(request.ExecutionWorkspace)
	request.ExecutionHeadBranch = util.SanitizeTextForPostgres(request.ExecutionHeadBranch)
	request.ExecutionHeadSHA = util.SanitizeTextForPostgres(request.ExecutionHeadSHA)
	request.ExecutionHeadState = util.SanitizeTextForPostgres(request.ExecutionHeadState)
	if task.Status != "running" && !(task.Status == "cancelled" && request.Finished) {
		writeError(w, http.StatusConflict, "execution provenance requires a running task")
		return
	}
	if request.Finished && !terminalTaskStatus(task.Status) {
		writeError(w, http.StatusConflict, "finished execution provenance requires a terminal task")
		return
	}

	facts := workProductExecutionFacts{
		repoIdentity:       request.ExecutionRepoIdentity,
		executionWorkspace: request.ExecutionWorkspace,
		headBranch:         request.ExecutionHeadBranch,
		headSHA:            request.ExecutionHeadSHA,
		headState:          request.ExecutionHeadState,
	}
	values, err := normalizeWorkProductExecutionFacts(facts, task)
	if err != nil {
		writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_provenance", err.Error())
		return
	}
	provenance, err := h.upsertWorkProductExecutionProvenance(r.Context(), task, parseUUID(workspaceID), values, request.Finished)
	if err != nil {
		if isCheckViolation(err) || isUniqueViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_provenance", "provenance violates a database constraint")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	if request.Finished {
		if err := h.scheduleWorkProductDiscovery(r.Context(), task, parseUUID(workspaceID), facts); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to schedule work product discovery")
			return
		}
	}
	writeJSON(w, http.StatusOK, provenance)
}

func (h *Handler) upsertWorkProductExecutionProvenance(ctx context.Context, task db.AgentTaskQueue, workspaceID pgtype.UUID, values workProductProvenanceValues, finished bool) (db.AgentTaskExecutionProvenance, error) {
	if h.DB == nil {
		return db.AgentTaskExecutionProvenance{}, errors.New("database unavailable")
	}
	return upsertWorkProductExecutionProvenance(ctx, h.DB, task, workspaceID, values, finished)
}

func upsertWorkProductExecutionProvenance(ctx context.Context, executor dbExecutor, task db.AgentTaskQueue, workspaceID pgtype.UUID, values workProductProvenanceValues, finished bool) (db.AgentTaskExecutionProvenance, error) {
	finishedAt := any(nil)
	if finished {
		finishedAt = time.Now()
	}
	discoveryStatus := "not_attempted"
	if finished {
		discoveryStatus = "pending"
	}
	return scanWorkProductProvenance(executor.QueryRow(ctx, `
INSERT INTO agent_task_execution_provenance (
    workspace_id, task_id, run_id, repo_identity, execution_workspace,
    head_branch, head_sha, head_state, started_at, finished_at, discovery_status
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), $9, $10)
ON CONFLICT (workspace_id, task_id, repo_identity, execution_workspace) DO UPDATE SET
    run_id = COALESCE(agent_task_execution_provenance.run_id, EXCLUDED.run_id),
    head_branch = CASE
        WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_branch
        ELSE COALESCE(agent_task_execution_provenance.head_branch, EXCLUDED.head_branch)
    END,
    head_sha = CASE
        WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_sha
        ELSE COALESCE(agent_task_execution_provenance.head_sha, EXCLUDED.head_sha)
    END,
    head_state = CASE
        WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_state
        ELSE agent_task_execution_provenance.head_state
    END,
    started_at = COALESCE(agent_task_execution_provenance.started_at, EXCLUDED.started_at),
    finished_at = CASE
        WHEN $9::timestamptz IS NOT NULL THEN COALESCE(agent_task_execution_provenance.finished_at, $9::timestamptz)
        ELSE agent_task_execution_provenance.finished_at
    END,
    discovery_status = CASE
        WHEN $11::boolean AND agent_task_execution_provenance.discovery_status IN ('not_attempted', 'pending', 'in_progress') THEN 'pending'
        ELSE agent_task_execution_provenance.discovery_status
    END,
    discovery_lease_id = CASE
        WHEN $11::boolean AND agent_task_execution_provenance.discovery_status IN ('not_attempted', 'pending', 'in_progress') THEN NULL
        ELSE agent_task_execution_provenance.discovery_lease_id
    END,
    discovery_match_count = CASE
        WHEN $11::boolean AND agent_task_execution_provenance.discovery_status IN ('not_attempted', 'pending', 'in_progress') THEN 0
        ELSE agent_task_execution_provenance.discovery_match_count
    END,
    discovery_reason = CASE
        WHEN $11::boolean AND agent_task_execution_provenance.discovery_status IN ('not_attempted', 'pending', 'in_progress') THEN NULL
        ELSE agent_task_execution_provenance.discovery_reason
    END,
    discovery_work_product_id = CASE
        WHEN $11::boolean AND agent_task_execution_provenance.discovery_status IN ('not_attempted', 'pending', 'in_progress') THEN NULL
        ELSE agent_task_execution_provenance.discovery_work_product_id
    END,
    discovery_at = CASE
        WHEN $11::boolean AND agent_task_execution_provenance.discovery_status IN ('not_attempted', 'pending', 'in_progress') THEN NULL
        ELSE agent_task_execution_provenance.discovery_at
    END,
    updated_at = now()
RETURNING `+workProductProvenanceColumns,
		workspaceID,
		task.ID,
		nullableWorkProductUUID(task.AutomationRunID),
		values.RepoIdentity,
		values.ExecutionWorkspace,
		nullableWorkProductText(valueOrEmpty(values.HeadBranch)),
		nullableWorkProductText(valueOrEmpty(values.HeadSHA)),
		values.HeadState,
		finishedAt,
		discoveryStatus,
		finished,
	))
}

func (r *WorkProductDiscoveryRuntime) discoverTask(ctx context.Context, task db.AgentTaskQueue, workspaceID pgtype.UUID) error {
	if !terminalTaskStatus(task.Status) {
		return nil
	}
	if skipped, err := r.markExplicitRelation(ctx, workspaceID, task.ID); err != nil {
		return err
	} else if skipped {
		return nil
	}
	items, err := r.handler.Queries.ListExecutionProvenanceByTask(ctx, db.ListExecutionProvenanceByTaskParams{
		WorkspaceID: workspaceID,
		TaskID:      task.ID,
	})
	if err != nil {
		return err
	}
	for _, item := range items {
		claimed, ok, claimErr := r.claim(ctx, item)
		if claimErr != nil {
			return claimErr
		}
		if !ok {
			continue
		}
		if err := r.discoverOne(ctx, task, claimed); err != nil {
			// Leave the lease stale on infrastructure/provider errors. The next
			// sweep can reclaim it without converting an outage into a false
			// "unassociated" verdict.
			slog.Warn("work product discovery: item failed", "task_id", uuidToString(task.ID), "repo", claimed.RepoIdentity, "error", err)
		}
	}
	return nil
}

func (r *WorkProductDiscoveryRuntime) claim(ctx context.Context, item db.AgentTaskExecutionProvenance) (db.AgentTaskExecutionProvenance, bool, error) {
	claimed, err := scanWorkProductProvenance(r.handler.DB.QueryRow(ctx, `
UPDATE agent_task_execution_provenance
SET discovery_status = 'in_progress',
    discovery_lease_id = gen_random_uuid(),
    discovery_at = now(),
    updated_at = now()
WHERE workspace_id = $1 AND task_id = $2 AND repo_identity = $3 AND execution_workspace = $4
  AND (discovery_status = 'pending' OR (discovery_status = 'in_progress' AND updated_at < now() - $5::interval))
RETURNING `+workProductProvenanceColumns,
		item.WorkspaceID,
		item.TaskID,
		item.RepoIdentity,
		item.ExecutionWorkspace,
		workProductDiscoveryLease,
	))
	if err != nil {
		if isNotFound(err) {
			return db.AgentTaskExecutionProvenance{}, false, nil
		}
		return db.AgentTaskExecutionProvenance{}, false, err
	}
	return claimed, true, nil
}

func (r *WorkProductDiscoveryRuntime) discoverOne(ctx context.Context, task db.AgentTaskQueue, item db.AgentTaskExecutionProvenance) error {
	finish := func(status, reason string, count int32, productID pgtype.UUID) error {
		return r.record(ctx, item, status, count, reason, productID)
	}

	repoIdentity, repoOK := normalizeWorkProductRepoIdentity(item.RepoIdentity)
	if !repoOK {
		return finish("ineligible", "invalid_repository_identity", 0, pgtype.UUID{})
	}
	if !taskExecutionWorkspaceMatches(textWorkProductValue(task.WorkDir), textWorkProductValue(task.DurableWorkDir), item.ExecutionWorkspace) {
		return finish("ineligible", "execution_workspace_not_owned", 0, pgtype.UUID{})
	}
	branch := textWorkProductValue(item.HeadBranch)
	sha := textWorkProductValue(item.HeadSha)
	if item.HeadState != "attached" {
		decision := classifyBranchDiscovery(item.HeadState, 0, 0)
		return finish(decision.Status, decision.Reason, 0, pgtype.UUID{})
	}
	if branch == "" {
		return finish("ineligible", "missing_head_branch", 0, pgtype.UUID{})
	}
	if sha == "" {
		return finish("ineligible", "missing_head_sha", 0, pgtype.UUID{})
	}
	if !task.AgentID.Valid {
		return finish("ineligible", "missing_task_agent", 0, pgtype.UUID{})
	}
	workspace, err := r.handler.Queries.GetWorkspace(ctx, item.WorkspaceID)
	if err != nil {
		if isNotFound(err) {
			return finish("ineligible", "workspace_not_found", 0, pgtype.UUID{})
		}
		return err
	}
	if authorized, err := r.taskRepositoryAuthorized(ctx, task, workspace, repoIdentity); err != nil {
		return err
	} else if !authorized {
		return finish("ineligible", "repository_not_authorized", 0, pgtype.UUID{})
	}
	parts := strings.Split(repoIdentity, "/")
	if len(parts) != 2 {
		return finish("ineligible", "invalid_repository_identity", 0, pgtype.UUID{})
	}
	if r.handler.PRRefresh == nil || !r.handler.PRRefresh.Enabled() {
		return finish("ineligible", "github_app_not_configured", 0, pgtype.UUID{})
	}
	installations, err := r.handler.Queries.ListGitHubInstallationsByWorkspace(ctx, item.WorkspaceID)
	if err != nil {
		return err
	}
	if len(installations) == 0 {
		return finish("ineligible", "github_installation_not_found", 0, pgtype.UUID{})
	}

	matches := make([]ghsnapshot.PullRequestHeadMatch, 0)
	lookupErr := false
	mismatch := false
	seenNumbers := make(map[int32]struct{})
	matchInstallationIDs := make(map[int32]int64)
	for _, installation := range installations {
		found, lookupError := r.handler.PRRefresh.PullRequestsByHead(ctx, installation.InstallationID, parts[0], parts[1], branch)
		if lookupError != nil {
			lookupErr = true
			continue
		}
		for _, candidate := range found {
			metadata := candidate.Metadata
			if metadata.Branch != branch || !headRepositoryMatches(metadata.HeadRepoIdentity, repoIdentity) || !strings.EqualFold(metadata.HeadSHA, sha) {
				mismatch = true
				continue
			}
			if _, seen := seenNumbers[candidate.Number]; seen {
				continue
			}
			seenNumbers[candidate.Number] = struct{}{}
			matchInstallationIDs[candidate.Number] = installation.InstallationID
			matches = append(matches, candidate)
		}
	}
	sort.Slice(matches, func(i, j int) bool {
		return matches[i].Number < matches[j].Number
	})
	if lookupErr {
		return finish("ambiguous", "github_lookup_failed", int32(len(matches)), pgtype.UUID{})
	}
	if len(matches) == 0 && mismatch {
		return finish("ambiguous", "pull_request_head_mismatch", 0, pgtype.UUID{})
	}

	if r.handler.TxStarter == nil {
		return errors.New("database unavailable")
	}
	tx, err := r.handler.TxStarter.Begin(ctx)
	if err != nil {
		return err
	}
	rollback := func() { _ = tx.Rollback(ctx) }
	// The issue lock is taken before the branch/task advisory locks, matching
	// explicit relation creation and preventing a discovery/manual race from
	// producing two active relation rows.
	if task.IssueID.Valid {
		var lockedID pgtype.UUID
		if err := tx.QueryRow(ctx, `SELECT id FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE`, task.IssueID, item.WorkspaceID).Scan(&lockedID); err != nil {
			rollback()
			if isNotFound(err) {
				return finish("ineligible", "issue_not_found", 0, pgtype.UUID{})
			}
			return err
		}
	}
	if _, err := tx.Exec(ctx, `SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2 || ':' || $3, 0))`, item.WorkspaceID, repoIdentity, branch); err != nil {
		rollback()
		return err
	}
	if _, err := tx.Exec(ctx, `SELECT pg_advisory_xact_lock(hashtextextended($1::text || ':' || $2::text, 0))`, item.WorkspaceID, item.TaskID); err != nil {
		rollback()
		return err
	}
	lockedItem, err := scanWorkProductProvenance(tx.QueryRow(ctx, `SELECT `+workProductProvenanceColumns+` FROM agent_task_execution_provenance WHERE workspace_id = $1 AND task_id = $2 AND repo_identity = $3 AND execution_workspace = $4 AND discovery_status = 'in_progress' AND discovery_lease_id = $5 FOR UPDATE`, item.WorkspaceID, item.TaskID, item.RepoIdentity, item.ExecutionWorkspace, item.DiscoveryLeaseID))
	if err != nil {
		rollback()
		if isNotFound(err) {
			return nil
		}
		return err
	}
	item = lockedItem
	var otherExecutionCount int64
	if err := tx.QueryRow(ctx, `
SELECT count(*)
FROM agent_task_execution_provenance
WHERE workspace_id = $1 AND repo_identity = $2 AND head_branch = $3
  AND task_id <> $4
  AND (finished_at IS NULL OR head_sha = $5)`, item.WorkspaceID, repoIdentity, branch, item.TaskID, sha).Scan(&otherExecutionCount); err != nil {
		rollback()
		return err
	}
	var explicitProductID pgtype.UUID
	explicitErr := tx.QueryRow(ctx, `SELECT work_product_id FROM work_product_relation WHERE workspace_id = $1 AND task_id = $2 AND relation_source = 'task_explicit' AND detached_at IS NULL ORDER BY attached_at ASC, id ASC LIMIT 1`, item.WorkspaceID, item.TaskID).Scan(&explicitProductID)
	if explicitErr == nil {
		if err := recordWorkProductDiscoveryExec(ctx, tx, item, "associated", 1, "explicit_relation_exists", explicitProductID); err != nil {
			rollback()
			return err
		}
		if err := tx.Commit(ctx); err != nil {
			rollback()
			return err
		}
		return nil
	}
	if !isNotFound(explicitErr) {
		rollback()
		return explicitErr
	}

	decision := classifyBranchDiscovery(item.HeadState, int(otherExecutionCount), len(matches))
	if decision.Status != "associated" {
		if !lookupErr && decision.Status == "unassociated" && mismatch {
			decision.Status = "ambiguous"
			decision.Reason = "pull_request_head_mismatch"
		}
		if err := recordWorkProductDiscoveryExec(ctx, tx, item, decision.Status, int32(len(matches)), decision.Reason, pgtype.UUID{}); err != nil {
			rollback()
			return err
		}
		if err := tx.Commit(ctx); err != nil {
			rollback()
			return err
		}
		return nil
	}

	selected := matches[0]
	mirrored, err := r.handler.Queries.WithTx(tx).UpsertGitHubPullRequest(ctx, githubPullRequestUpsertParams(item.WorkspaceID, matchInstallationIDs[selected.Number], parts[0], parts[1], selected.Metadata))
	if err != nil {
		rollback()
		return err
	}
	product, err := r.handler.Queries.WithTx(tx).CreateWorkProduct(ctx, db.CreateWorkProductParams{
		WorkspaceID:        item.WorkspaceID,
		Kind:               "pull_request",
		Provider:           "github",
		ExternalIdentity:   strings.ToLower(repoIdentity) + "#" + strconv.Itoa(int(selected.Number)),
		ExternalUrl:        pgtype.Text{String: selected.Metadata.HTMLURL, Valid: selected.Metadata.HTMLURL != ""},
		ProviderRecordType: pgtype.Text{String: "github_pull_request", Valid: true},
		ProviderRecordID:   mirrored.ID,
	})
	if err != nil {
		rollback()
		return err
	}
	relation, err := scanWorkProductRelation(tx.QueryRow(ctx, `
INSERT INTO work_product_relation (
    workspace_id, work_product_id, issue_id, task_id, run_id, relation_key,
    relation_source, attached_by_type, attached_by_id, close_intent
)
VALUES ($1, $2, $3, $4, $5, $6, 'execution_branch_discovery', 'agent', $7, FALSE)
ON CONFLICT (work_product_id, relation_key) WHERE detached_at IS NULL DO UPDATE SET
    relation_source = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.relation_source
        ELSE work_product_relation.relation_source
    END,
    attached_by_type = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.attached_by_type
        ELSE work_product_relation.attached_by_type
    END,
    attached_by_id = CASE
        WHEN work_product_relation.relation_source = 'execution_branch_discovery'
         AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
        THEN EXCLUDED.attached_by_id
        ELSE work_product_relation.attached_by_id
    END,
    close_intent = work_product_relation.close_intent OR EXCLUDED.close_intent
RETURNING `+workProductRelationColumns,
		item.WorkspaceID,
		product.ID,
		nullableWorkProductUUID(task.IssueID),
		task.ID,
		nullableWorkProductUUID(task.AutomationRunID),
		workProductRelationKey(task.IssueID, task.ID, task.AutomationRunID),
		task.AgentID,
	))
	if err != nil {
		rollback()
		return err
	}
	if err := recordWorkProductDiscoveryExec(ctx, tx, item, "associated", int32(len(matches)), "unique_pull_request_for_exact_head", product.ID); err != nil {
		rollback()
		return err
	}
	if err := tx.Commit(ctx); err != nil {
		rollback()
		return err
	}
	r.publishDiscovery(item, task, mirrored, relation)
	return nil
}

func githubPullRequestUpsertParams(workspaceID pgtype.UUID, installationID int64, owner, repo string, metadata ghsnapshot.PullRequestMetadata) db.UpsertGitHubPullRequestParams {
	return db.UpsertGitHubPullRequestParams{
		WorkspaceID:         workspaceID,
		InstallationID:      installationID,
		RepoOwner:           owner,
		RepoName:            repo,
		PrNumber:            metadata.Number,
		Title:               metadata.Title,
		State:               metadata.State,
		HtmlUrl:             metadata.HTMLURL,
		PrCreatedAt:         workProductTimestamp(metadata.CreatedAt),
		PrUpdatedAt:         workProductTimestamp(metadata.UpdatedAt),
		HeadSha:             metadata.HeadSHA,
		Additions:           metadata.Additions,
		Deletions:           metadata.Deletions,
		ChangedFiles:        metadata.ChangedFiles,
		Branch:              pgtype.Text{String: metadata.Branch, Valid: metadata.Branch != ""},
		AuthorLogin:         pgtype.Text{String: metadata.AuthorLogin, Valid: metadata.AuthorLogin != ""},
		AuthorAvatarUrl:     pgtype.Text{String: metadata.AuthorAvatarURL, Valid: metadata.AuthorAvatarURL != ""},
		MergedAt:            workProductOptionalTimestamp(metadata.MergedAt),
		ClosedAt:            workProductOptionalTimestamp(metadata.ClosedAt),
		ClearMergeableState: pgtype.Bool{Bool: false, Valid: true},
	}
}

func workProductTimestamp(value time.Time) pgtype.Timestamptz {
	return pgtype.Timestamptz{Time: value, Valid: !value.IsZero()}
}

func workProductOptionalTimestamp(value *time.Time) pgtype.Timestamptz {
	if value == nil {
		return pgtype.Timestamptz{}
	}
	return workProductTimestamp(*value)
}

func (r *WorkProductDiscoveryRuntime) taskRepositoryAuthorized(ctx context.Context, task db.AgentTaskQueue, workspace db.Workspace, candidate string) (bool, error) {
	if workspaceContainsRepo(workspace.Repos, candidate) {
		return true, nil
	}
	projectIDs := make(map[string]pgtype.UUID)
	if task.IssueID.Valid {
		issue, err := r.handler.Queries.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID: task.IssueID, WorkspaceID: workspace.ID})
		if err != nil && !isNotFound(err) {
			return false, err
		}
		if err == nil && issue.ProjectID.Valid {
			projectIDs[uuidToString(issue.ProjectID)] = issue.ProjectID
		}
	}
	if task.ChatSessionID.Valid {
		session, err := r.handler.Queries.GetChatSessionInWorkspace(ctx, db.GetChatSessionInWorkspaceParams{ID: task.ChatSessionID, WorkspaceID: workspace.ID})
		if err != nil && !isNotFound(err) {
			return false, err
		}
		if err == nil && session.ProjectID.Valid {
			projectIDs[uuidToString(session.ProjectID)] = session.ProjectID
		}
	}
	for _, projectID := range projectIDs {
		resources, err := r.handler.Queries.ListProjectResources(ctx, projectID)
		if err != nil {
			return false, err
		}
		for _, resource := range resources {
			if !sameWorkProductUUID(resource.WorkspaceID, workspace.ID) || resource.ResourceType != "github_repo" {
				continue
			}
			var ref struct {
				URL string `json:"url"`
			}
			if json.Unmarshal(resource.ResourceRef, &ref) == nil && ref.URL != "" {
				if identity, ok := normalizeWorkProductRepoIdentity(ref.URL); ok && identity == candidate {
					return true, nil
				}
			}
		}
	}
	return false, nil
}

func (r *WorkProductDiscoveryRuntime) markPending(ctx context.Context, workspaceID, taskID pgtype.UUID) error {
	if r.handler.DB == nil {
		return errors.New("database unavailable")
	}
	return markPendingWorkProductDiscovery(ctx, r.handler.DB, workspaceID, taskID)
}

func markPendingWorkProductDiscovery(ctx context.Context, executor dbExecutor, workspaceID, taskID pgtype.UUID) error {
	_, err := executor.Exec(ctx, `
UPDATE agent_task_execution_provenance
SET discovery_status = 'pending',
    discovery_lease_id = NULL,
    discovery_match_count = 0,
    discovery_reason = NULL,
    discovery_work_product_id = NULL,
    discovery_at = NULL,
    finished_at = COALESCE(finished_at, now()),
    updated_at = now()
WHERE workspace_id = $1 AND task_id = $2 AND discovery_status = 'not_attempted'`, workspaceID, taskID)
	return err
}

func (r *WorkProductDiscoveryRuntime) markExplicitRelation(ctx context.Context, workspaceID, taskID pgtype.UUID) (bool, error) {
	if r.handler.DB == nil {
		return false, errors.New("database unavailable")
	}
	return markExplicitWorkProductRelation(ctx, r.handler.DB, workspaceID, taskID)
}

func markExplicitWorkProductRelation(ctx context.Context, executor dbExecutor, workspaceID, taskID pgtype.UUID) (bool, error) {
	tag, err := executor.Exec(ctx, `
WITH explicit_relation AS (
    SELECT work_product_id
    FROM work_product_relation
    WHERE workspace_id = $1 AND task_id = $2
      AND relation_source = 'task_explicit' AND detached_at IS NULL
    ORDER BY attached_at ASC, id ASC
    LIMIT 1
)
UPDATE agent_task_execution_provenance AS provenance
SET discovery_status = 'associated',
    discovery_lease_id = NULL,
    discovery_match_count = 1,
    discovery_reason = 'explicit_relation_exists',
    discovery_work_product_id = explicit_relation.work_product_id,
    discovery_at = now(),
    updated_at = now()
FROM explicit_relation
WHERE provenance.workspace_id = $1 AND provenance.task_id = $2
  AND provenance.discovery_status IN ('not_attempted', 'pending', 'in_progress')`, workspaceID, taskID)
	return tag.RowsAffected() > 0, err
}

func (r *WorkProductDiscoveryRuntime) markMissingTask(ctx context.Context, workspaceID, taskID pgtype.UUID) error {
	_, err := r.handler.DB.Exec(ctx, `
UPDATE agent_task_execution_provenance
SET discovery_status = 'ineligible',
    discovery_lease_id = NULL,
    discovery_reason = 'task_not_found',
    discovery_at = now(),
    updated_at = now()
WHERE workspace_id = $1 AND task_id = $2
  AND discovery_status IN ('not_attempted', 'pending', 'in_progress')`, workspaceID, taskID)
	return err
}

var errWorkProductDiscoveryLeaseLost = errors.New("work product discovery lease lost")

func (r *WorkProductDiscoveryRuntime) record(ctx context.Context, item db.AgentTaskExecutionProvenance, status string, matchCount int32, reason string, productID pgtype.UUID) error {
	return recordWorkProductDiscoveryExec(ctx, r.handler.DB, item, status, matchCount, reason, productID)
}

func recordWorkProductDiscoveryExec(ctx context.Context, executor dbExecutor, item db.AgentTaskExecutionProvenance, status string, matchCount int32, reason string, productID pgtype.UUID) error {
	if status != "unassociated" && status != "ambiguous" && status != "associated" && status != "ineligible" {
		return errors.New("invalid discovery status")
	}
	if matchCount < 0 {
		return errors.New("invalid discovery match count")
	}
	tag, err := executor.Exec(ctx, `
UPDATE agent_task_execution_provenance
SET discovery_status = $3,
    discovery_match_count = $4,
    discovery_reason = $5,
    discovery_work_product_id = $6,
    discovery_lease_id = NULL,
    discovery_at = now(),
    updated_at = now()
WHERE workspace_id = $1 AND task_id = $2
  AND repo_identity = $7 AND execution_workspace = $8
  AND discovery_status = 'in_progress'
  AND discovery_lease_id = $9`, item.WorkspaceID, item.TaskID, status, matchCount,
		nullableWorkProductText(reason), nullableWorkProductUUID(productID), item.RepoIdentity, item.ExecutionWorkspace, item.DiscoveryLeaseID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() != 1 {
		return errWorkProductDiscoveryLeaseLost
	}
	return nil
}

func (r *WorkProductDiscoveryRuntime) publishDiscovery(item db.AgentTaskExecutionProvenance, task db.AgentTaskQueue, pr db.GithubPullRequest, relation db.WorkProductRelation) {
	if r.handler.Bus == nil {
		return
	}
	linkedIssueIDs := []string{}
	if task.IssueID.Valid {
		linkedIssueIDs = append(linkedIssueIDs, uuidToString(task.IssueID))
	}
	snapshotEnabled := r.handler.PRRefresh != nil && r.handler.PRRefresh.Enabled()
	r.handler.publish(protocol.EventPullRequestUpdated, uuidToString(item.WorkspaceID), "agent", uuidToString(task.AgentID), map[string]any{
		"pull_request":     githubPullRequestToResponse(pr, snapshotEnabled),
		"linked_issue_ids": linkedIssueIDs,
		"relation":         relation,
	})
}
