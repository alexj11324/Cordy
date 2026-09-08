package daemon

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/cli"
	"github.com/orvilo-ai/orvilo/server/internal/daemon/terminalreport"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
	"github.com/orvilo-ai/orvilo/server/pkg/taskfailure"
)

func (d *Daemon) openTerminalReports() error {
	profileDir, err := cli.ProfileDir(d.cfg.Profile)
	if err != nil {
		return err
	}
	store, err := terminalreport.Open(terminalreport.Directory(profileDir, d.cfg.ServerBaseURL))
	if err != nil {
		return err
	}
	d.terminalStore = store
	d.terminalSender = terminalreport.NewSender(store, d.client, d.logger)
	d.terminalSender.SetPermanentRejectionHandler(d.handlePermanentTerminalReport)
	return nil
}

type terminalReportBody struct {
	Output                string `json:"output"`
	Error                 string `json:"error"`
	BranchName            string `json:"branch_name"`
	SessionID             string `json:"session_id"`
	WorkDir               string `json:"work_dir"`
	DurableWorkDir        string `json:"durable_work_dir"`
	FailureReason         string `json:"failure_reason"`
	SessionRolloutMissing bool   `json:"session_rollout_missing"`
	RetiredSessionID      string `json:"retired_session_id"`
	ExecutionRepoIdentity string `json:"execution_repo_identity"`
	ExecutionWorkspace    string `json:"execution_workspace"`
	ExecutionHeadBranch   string `json:"execution_head_branch"`
	ExecutionHeadSHA      string `json:"execution_head_sha"`
	ExecutionHeadState    string `json:"execution_head_state"`
}

// handlePermanentTerminalReport settles an accepted execution whose complete
// report was rejected by the server. A permanently rejected failure report has
// no safer alternate transition, so it stays pending and blocks new claims for
// operator recovery; silently renaming it would strand the running task.
func (d *Daemon) handlePermanentTerminalReport(ctx context.Context, report terminalreport.Report, rejection error) error {
	var body terminalReportBody
	if err := json.Unmarshal(report.Body, &body); err != nil {
		d.terminalDeliveryBlocked.Store(true)
		return fmt.Errorf("decode terminal report: %w", err)
	}
	if report.Kind != "complete" {
		d.terminalDeliveryBlocked.Store(true)
		return fmt.Errorf("permanently rejected %s report requires manual recovery: %w", report.Kind, rejection)
	}

	message := fmt.Sprintf("complete task report rejected by server: %s", rejection)
	provenance := ExecutionProvenanceReport{
		RepoIdentity:       body.ExecutionRepoIdentity,
		ExecutionWorkspace: body.ExecutionWorkspace,
		HeadBranch:         body.ExecutionHeadBranch,
		HeadSHA:            body.ExecutionHeadSHA,
		HeadState:          body.ExecutionHeadState,
	}
	settleCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), terminalTaskReportTimeout)
	defer cancel()
	if err := d.client.FailTaskWithProvenance(
		settleCtx, report.TaskID, message, body.SessionID, body.WorkDir,
		body.BranchName, taskfailure.Classify(message).String(), body.SessionRolloutMissing,
		body.RetiredSessionID, body.DurableWorkDir, provenance,
	); err != nil {
		d.terminalDeliveryBlocked.Store(true)
		return fmt.Errorf("fallback failure callback: %w", err)
	}
	return nil
}

func (d *Daemon) persistTerminalReport(report terminalTaskReport) error {
	if d.terminalStore == nil || d.terminalSender == nil {
		return errors.New("terminal report store is not initialized")
	}
	body := map[string]any{
		"branch_name": report.branchName, "session_id": report.sessionID,
		"work_dir": report.workDir, "durable_work_dir": report.durableWorkDir,
		"session_rollout_missing": report.sessionRolloutMissing,
		"retired_session_id":      report.retiredSessionID,
	}
	var kind string
	switch report.kind {
	case terminalTaskReportComplete:
		kind = "complete"
		body["output"] = report.output
	case terminalTaskReportFail:
		kind = "fail"
		body["error"] = report.errorMessage
		body["failure_reason"] = report.failureReason
	default:
		return fmt.Errorf("unsupported terminal task report kind %d", report.kind)
	}
	for key, value := range executionProvenanceBody(report.executionProvenance) {
		body[key] = value
	}
	data, err := json.Marshal(body)
	if err != nil {
		return err
	}
	saved, err := terminalreport.NewReport(report.taskID, report.claimFence, kind, data, time.Now())
	if err != nil {
		return err
	}
	if saved, err = d.terminalStore.Save(saved); err != nil {
		return err
	}
	d.logger.Info("terminal result persisted awaiting acknowledgement", "task_id", report.taskID, "report_id", saved.Identity.ReportID)
	d.terminalSender.Wake()
	return nil
}

func (c *Client) SendTerminalReport(ctx context.Context, report terminalreport.Report) (protocol.TerminalReportAck, error) {
	var body map[string]json.RawMessage
	if err := json.Unmarshal(report.Body, &body); err != nil {
		return protocol.TerminalReportAck{}, err
	}
	identity, err := json.Marshal(report.Identity)
	if err != nil {
		return protocol.TerminalReportAck{}, err
	}
	body["terminal_report"] = identity
	var ack protocol.TerminalReportAck
	err = c.postJSON(ctx, fmt.Sprintf("/api/daemon/tasks/%s/%s", report.TaskID, report.Kind), body, &ack)
	var response *requestError
	if errors.As(err, &response) {
		// Only receipt protocol rejections are permanent. 401/403 can recover
		// after credential refresh; an unavailable endpoint can recover after
		// deployment. Keep those reports until the server can answer them.
		if response.StatusCode == http.StatusConflict || response.StatusCode == http.StatusBadRequest || (response.StatusCode == http.StatusNotFound && isTaskNotFoundError(err)) {
			return ack, &terminalreport.PermanentError{Err: err}
		}
	}
	return ack, err
}

type terminalTaskReportKind uint8

const (
	terminalTaskReportComplete terminalTaskReportKind = iota + 1
	terminalTaskReportFail

	// CompleteTask and FailTask can make six 30-second HTTP attempts around
	// the five backoffs in defaultTerminalRetrySchedule (124 seconds total).
	// Keep the detached callback's own deadline above that worst-case budget
	// so it does not silently shorten the client's existing retry contract.
	// During daemon restart pollLoop still imposes its separate 30-second
	// process drain boundary.
	terminalTaskReportTimeout = 6 * time.Minute
)

// terminalTaskReport is the single daemon-side representation of a terminal
// callback. Keeping every complete/fail path behind this value and
// reportTerminalTask gives the durable outbox one insertion point without
// revisiting every task exit when it is added.
type terminalTaskReport struct {
	kind           terminalTaskReportKind
	taskID         string
	claimFence     string
	output         string
	branchName     string
	errorMessage   string
	sessionID      string
	workDir        string
	durableWorkDir string
	failureReason  string
	// sessionRolloutMissing is true when the daemon withheld this task's Codex
	// session because its rollout was not in the store (MUL-5305). The server
	// clears the resume pointer and flags the continuity gap for the next claim.
	sessionRolloutMissing bool
	// retiredSessionID names a session this run was told to resume and then
	// abandoned as unresumable (GH #6066). The server records it so no later
	// run on the issue or chat can select it again, however many clean rows
	// still reference it.
	retiredSessionID    string
	executionProvenance ExecutionProvenanceReport
}

// reportTaskResult writes the final task disposition back to the server.
//
// Fail closed: only an explicit "completed" status is reported as success.
// Anything else — "blocked", "cancelled", or any future status we forget to
// enumerate — must go through FailTask, so a run that never produced a real
// result can never be displayed as "Completed" in the UI (e.g. provider 429 /
// out-of-credit / runtime crash). Forward SessionID/WorkDir on every path:
// the agent may have built a real session before getting stuck, and we want
// the next chat turn to resume there rather than start over and "forget"
// the conversation.
func (d *Daemon) reportTaskResult(ctx context.Context, task Task, result TaskResult, taskLog *slog.Logger) {
	taskID := task.ID
	switch result.Status {
	case "completed":
		taskLog.Info("task completed", "status", result.Status)
		err := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:                  terminalTaskReportComplete,
			taskID:                taskID,
			claimFence:            task.ClaimFence,
			output:                result.Comment,
			branchName:            result.BranchName,
			sessionID:             result.SessionID,
			workDir:               result.WorkDir,
			durableWorkDir:        result.DurableWorkDir,
			sessionRolloutMissing: result.SessionRolloutMissing,
			retiredSessionID:      result.RetiredSessionID,
			executionProvenance:   executionProvenanceFromTaskResult(result),
		})
		if err == nil {
			return
		}
		if task.ClaimFence != "" {
			taskLog.Error("terminal result was not persisted; claims paused", "error", err)
			return
		}
		// CompleteTask retries transient errors internally. A transient
		// error reaching us here means the schedule was exhausted while
		// the upstream was still 5xx / unreachable. Converting that into
		// a fail would lose the agent's actual result and surface a
		// misleading red badge in the UI — leave the task in running
		// instead so a future fix (server-side stuck-task reaper, or a
		// daemon-side persistent pending queue) can recover it. Only
		// permanent server-side rejections (4xx other than 408/429)
		// warrant the legacy fallback, because at that point the server
		// has already refused this task and the only useful UI signal
		// left is a concrete failure.
		if isTransientError(err) {
			taskLog.Error("complete task failed after retries; leaving task in running rather than falling back to fail", "error", err)
			return
		}
		taskLog.Error("complete task rejected by server, falling back to fail", "error", err)
		// MUL-2946: this fallback fires when a server-side complete
		// callback was permanently rejected (4xx other than 408/429)
		// — the agent itself succeeded, so the err here describes the
		// server response rather than an agent failure. The classifier
		// is unlikely to match anything in the server's error text and
		// will land at ReasonAgentUnknown ("agent_error.unknown"),
		// which is the canonical replacement for the legacy
		// "agent_error" coarse bucket.
		fallbackErrMsg := fmt.Sprintf("complete task failed: %s", err.Error())
		if failErr := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:         terminalTaskReportFail,
			taskID:       taskID,
			claimFence:   task.ClaimFence,
			errorMessage: fallbackErrMsg,
			// The agent succeeded here — only the server's complete callback was
			// rejected. Its branch is real and already committed, so it must
			// survive the downgrade to a failure report.
			branchName:            result.BranchName,
			sessionID:             result.SessionID,
			workDir:               result.WorkDir,
			durableWorkDir:        result.DurableWorkDir,
			failureReason:         taskfailure.Classify(fallbackErrMsg).String(),
			sessionRolloutMissing: result.SessionRolloutMissing,
			retiredSessionID:      result.RetiredSessionID,
			executionProvenance:   executionProvenanceFromTaskResult(result),
		}); failErr != nil {
			taskLog.Error("fail task fallback also failed", "error", failErr)
		}
	default:
		failureReason := result.FailureReason
		if failureReason == "" {
			if result.Status == "cancelled" {
				// "cancelled" is a deliberate non-failure terminal
				// state masquerading as a failure_reason — preserved
				// outside the canonical taxonomy so the UI can render
				// it differently from a real failure.
				failureReason = "cancelled"
			} else {
				// MUL-2946: classify the agent's comment text so the
				// failure_reason lands in the refined taxonomy
				// (provider_auth_or_access, context_overflow,
				// process_failure, …) instead of the legacy coarse
				// "agent_error" bucket. Empty comment lands in
				// ReasonAgentUnknown.
				failureReason = taskfailure.Classify(result.Comment).String()
			}
		}
		taskLog.Info("task did not complete, reporting failure", "status", result.Status, "failure_reason", failureReason)
		if err := d.reportTerminalTask(ctx, terminalTaskReport{
			kind:           terminalTaskReportFail,
			taskID:         taskID,
			claimFence:     task.ClaimFence,
			errorMessage:   result.Comment,
			sessionID:      result.SessionID,
			workDir:        result.WorkDir,
			durableWorkDir: result.DurableWorkDir,
			// Worktree mode commits the agent's leftovers before tearing the
			// worktree down, so a failed run routinely still has a branch. This
			// is the case where the user most needs it: the task went wrong and
			// they want to see how far it got.
			branchName:            result.BranchName,
			failureReason:         failureReason,
			sessionRolloutMissing: result.SessionRolloutMissing,
			retiredSessionID:      result.RetiredSessionID,
			executionProvenance:   executionProvenanceFromTaskResult(result),
		}); err != nil {
			taskLog.Error("report failed task failed", "error", err)
		}
	}
}

func executionProvenanceFromTaskResult(result TaskResult) ExecutionProvenanceReport {
	return ExecutionProvenanceReport{
		RepoIdentity:       result.ExecutionRepoIdentity,
		ExecutionWorkspace: result.ExecutionWorkspace,
		HeadBranch:         result.ExecutionHeadBranch,
		HeadSHA:            result.ExecutionHeadSHA,
		HeadState:          result.ExecutionHeadState,
	}
}

// reportTerminalTask is the only path that sends complete/fail callbacks.
// It deliberately preserves context values while discarding cancellation and
// parent deadlines: daemon shutdown cancels the root context before pollLoop's
// 30-second drain, but terminal callbacks must still use that remaining window.
// The explicit timeout keeps this detached work bounded during normal runs.
func (d *Daemon) reportTerminalTask(parentCtx context.Context, report terminalTaskReport) error {
	if report.claimFence != "" {
		if err := d.persistTerminalReport(report); err != nil {
			d.terminalPersistenceFailed.Store(true)
			return err
		}
		return nil
	}
	// A claim from an older server has no receipt protocol. Preserve its
	// callback contract until that server is upgraded.

	ctx, cancel := context.WithTimeout(context.WithoutCancel(parentCtx), terminalTaskReportTimeout)
	defer cancel()

	switch report.kind {
	case terminalTaskReportComplete:
		return d.client.CompleteTaskWithProvenance(ctx, report.taskID, report.output, report.branchName, report.sessionID, report.workDir, report.sessionRolloutMissing, report.retiredSessionID, report.durableWorkDir, report.executionProvenance)
	case terminalTaskReportFail:
		return d.client.FailTaskWithProvenance(ctx, report.taskID, report.errorMessage, report.sessionID, report.workDir, report.branchName, report.failureReason, report.sessionRolloutMissing, report.retiredSessionID, report.durableWorkDir, report.executionProvenance)
	default:
		return fmt.Errorf("unsupported terminal task report kind %d", report.kind)
	}
}
