package service

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/util"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
	"github.com/orvilo-ai/orvilo/server/pkg/dbid"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
	"github.com/orvilo-ai/orvilo/server/pkg/redact"
)

var (
	ErrTerminalReportConflict   = errors.New("terminal report conflicts with accepted result")
	ErrTerminalReportStaleClaim = errors.New("terminal report claim is no longer current")
	errTerminalReportReplay     = errors.New("terminal report already accepted")
)

// TerminalReport carries one validated identity and its committed receipt. Use a
// fresh instance for each delivery; callers must only send Ack after a nil error.
type TerminalReport struct {
	Identity protocol.TerminalReportIdentity
	Ack      *protocol.TerminalReportAck
	Replayed bool
	reportID pgtype.UUID
	fence    int64
	comment  *terminalReportComment
}

func NewTerminalReport(identity protocol.TerminalReportIdentity) (*TerminalReport, error) {
	reportID, err := util.ParseUUID(identity.ReportID)
	if err != nil || !reportID.Valid || reportID.Bytes == [16]byte{} {
		return nil, errors.New("invalid terminal report_id")
	}
	fence, err := strconv.ParseInt(identity.ClaimFence, 10, 64)
	if err != nil || fence <= 0 || strconv.FormatInt(fence, 10) != identity.ClaimFence {
		return nil, errors.New("invalid terminal claim_fence")
	}
	digest, err := hex.DecodeString(identity.PayloadSHA256)
	if err != nil || len(digest) != 32 || strings.ToLower(identity.PayloadSHA256) != identity.PayloadSHA256 {
		return nil, errors.New("invalid terminal payload_sha256")
	}
	identity.ReportID = util.UUIDToString(reportID)
	return &TerminalReport{Identity: identity, reportID: reportID, fence: fence}, nil
}

// checkTerminalReport runs after the chat-session lock and before any terminal
// writes. Holding the task row serializes concurrent deliveries and redispatch.
func checkTerminalReport(ctx context.Context, qtx *db.Queries, taskID pgtype.UUID, task *db.AgentTaskQueue, report *TerminalReport) error {
	if report == nil {
		return nil
	}
	current, err := qtx.LockTerminalReportTask(ctx, taskID)
	if err != nil {
		return err
	}
	if TaskClaimFence(current) != report.fence {
		return ErrTerminalReportStaleClaim
	}
	receipt, err := qtx.GetTerminalReportReceipt(ctx, db.GetTerminalReportReceiptParams{TaskID: taskID, ClaimFence: report.fence})
	if err == nil {
		if receipt.ReportID != report.reportID || receipt.PayloadSha256 != report.Identity.PayloadSHA256 {
			return ErrTerminalReportConflict
		}
		*task = current
		report.Ack = report.ack(taskID, receipt.TaskStatus)
		report.Replayed = true
		return errTerminalReportReplay
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return err
	}
	if current.Status != "running" && current.Status != "dispatched" && current.Status != "waiting_local_directory" {
		return ErrTerminalReportConflict
	}
	return qtx.LockTerminalReportIssue(ctx, taskID)
}

func (report *TerminalReport) ack(taskID pgtype.UUID, status string) *protocol.TerminalReportAck {
	return &protocol.TerminalReportAck{TerminalReportIdentity: report.Identity, TaskID: util.UUIDToString(taskID), Status: "accepted", TaskStatus: status}
}

func terminalReportHook(report *TerminalReport, hook TerminalTaskTxHook) TerminalTaskTxHook {
	if report == nil {
		return hook
	}
	return func(ctx context.Context, tx pgx.Tx, qtx *db.Queries, task db.AgentTaskQueue) error {
		if hook != nil {
			if err := hook(ctx, tx, qtx, task); err != nil {
				return err
			}
		}
		if err := qtx.CreateTerminalReportReceipt(ctx, db.CreateTerminalReportReceiptParams{
			ReportID: report.reportID, TaskID: task.ID, ClaimFence: report.fence, PayloadSha256: report.Identity.PayloadSHA256, TaskStatus: task.Status,
		}); err != nil {
			var pgErr *pgconn.PgError
			if errors.As(err, &pgErr) && pgErr.Code == "23505" {
				return ErrTerminalReportConflict
			}
			return fmt.Errorf("persist terminal report receipt: %w", err)
		}
		return nil
	}
}

// CompleteTaskWithTerminalReport atomically accepts a fenced terminal report.
func (s *TaskService) CompleteTaskWithTerminalReport(ctx context.Context, taskID pgtype.UUID, result []byte, sessionID, workDir, branchName string, sessionRolloutMissing bool, retiredSessionID, durableWorkDir string, hook TerminalTaskTxHook, report *TerminalReport) (*db.AgentTaskQueue, error) {
	return s.completeTask(ctx, taskID, result, sessionID, workDir, branchName, sessionRolloutMissing, retiredSessionID, durableWorkDir, hook, report)
}

// FailTaskWithTerminalReport uses the existing failure/retry transaction.
func (s *TaskService) FailTaskWithTerminalReport(ctx context.Context, taskID pgtype.UUID, errMsg, sessionID, workDir, branchName, failureReason string, sessionRolloutMissing bool, retiredSessionID, durableWorkDir string, hook TerminalTaskTxHook, report *TerminalReport) (*db.AgentTaskQueue, error) {
	return s.failTask(ctx, taskID, errMsg, sessionID, workDir, branchName, failureReason, sessionRolloutMissing, retiredSessionID, durableWorkDir, hook, report)
}

type terminalReportComment struct {
	issue   db.Issue
	created db.CreateCommentRow
	root    *db.Comment
}

func (s *TaskService) prepareTerminalReportComment(ctx context.Context, qtx *db.Queries, task db.AgentTaskQueue, result []byte, errMsg string, report *TerminalReport) error {
	if report == nil || !task.IssueID.Valid {
		return nil
	}
	content, kind := redact.Text(errMsg), "system"
	if task.Status == "completed" {
		suppressed, err := HasTeamLeaderNoActionEvaluationForTask(ctx, qtx, task)
		if err != nil {
			return err
		}
		commented, err := qtx.HasAgentCommentedSince(ctx, db.HasAgentCommentedSinceParams{IssueID: task.IssueID, AuthorID: task.AgentID, Since: task.StartedAt})
		if err != nil {
			return err
		}
		if suppressed || commented {
			return nil
		}
		var payload protocol.TaskCompletedPayload
		if err := json.Unmarshal(result, &payload); err != nil {
			return err
		}
		body := util.UnescapeBackslashEscapes(payload.Output)
		if task.TriggerCommentID.Valid && isTrivialDoneOutput(body) {
			return nil
		}
		content, kind = truncateFallbackCommentBody(redact.Text(body), maxSynthesizedFallbackCommentRunes), "comment"
	}
	if content == "" {
		return nil
	}
	issue, err := qtx.GetIssue(ctx, task.IssueID)
	if err != nil {
		return err
	}
	outcome := &terminalReportComment{issue: issue}
	if task.TriggerCommentID.Valid {
		root, err := qtx.GetThreadRoot(ctx, db.GetThreadRootParams{CommentID: task.TriggerCommentID, WorkspaceID: issue.WorkspaceID})
		if err != nil && !errors.Is(err, pgx.ErrNoRows) {
			return err
		}
		if err == nil {
			outcome.root = &root
		}
	}
	created, err := qtx.CreateComment(ctx, db.CreateCommentParams{ID: dbid.NewV7(), IssueID: task.IssueID, WorkspaceID: issue.WorkspaceID, AuthorType: "agent", AuthorID: task.AgentID, Content: content, Type: kind, ParentID: task.TriggerCommentID, SourceTaskID: task.ID})
	if err != nil {
		return err
	}
	outcome.created = created
	report.comment = outcome
	return nil
}

func (s *TaskService) publishTerminalReport(ctx context.Context, task db.AgentTaskQueue, report *TerminalReport) {
	if report == nil {
		return
	}
	report.Ack = report.ack(task.ID, task.Status)
	if outcome := report.comment; outcome != nil {
		s.publishAgentComment(ctx, outcome.issue, outcome.created, outcome.root)
	}
}
