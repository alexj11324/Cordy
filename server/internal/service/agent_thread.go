package service

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

const (
	maxAgentThreadMessageRunes = 12_000
	maxAgentThreadDepth        = 101 // root plus 100 continuations
)

type AgentThreadUnavailableReason string

const (
	AgentThreadProviderSessionRetired        AgentThreadUnavailableReason = "provider_session_retired"
	AgentThreadProviderSessionMissing        AgentThreadUnavailableReason = "provider_session_missing"
	AgentThreadFreshSessionRequired          AgentThreadUnavailableReason = "fresh_session_required"
	AgentThreadProviderSessionNotEstablished AgentThreadUnavailableReason = "provider_session_not_established"
	AgentThreadAgentArchived                 AgentThreadUnavailableReason = "agent_archived"
	AgentThreadAgentRuntimeUnbound           AgentThreadUnavailableReason = "agent_runtime_unbound"
	AgentThreadAgentRuntimeRebound           AgentThreadUnavailableReason = "agent_runtime_rebound"
	AgentThreadAgentRuntimeMissing           AgentThreadUnavailableReason = "agent_runtime_missing"
)

var (
	ErrAgentThreadIdempotencyConflict = errors.New("agent thread idempotency key was already used with different content")
	ErrAgentThreadDepthLimit          = errors.New("agent thread reached its maximum continuation depth")
	ErrAgentThreadInvokeForbidden     = errors.New("agent thread continuation is not permitted for this requester")
)

type AgentThreadUnavailableError struct {
	Reason AgentThreadUnavailableReason
}

func (e *AgentThreadUnavailableError) Error() string {
	return "agent thread is unavailable: " + string(e.Reason)
}

type AgentThreadContinuationReceipt struct {
	Task      db.AgentTaskQueue
	Coalesced bool
}

type agentThreadContext struct {
	Message string `json:"agent_thread_message"`
}

func AgentThreadMessage(task db.AgentTaskQueue) string {
	if len(task.Context) == 0 {
		return ""
	}
	var payload agentThreadContext
	if json.Unmarshal(task.Context, &payload) != nil {
		return ""
	}
	return payload.Message
}

func AgentThreadAvailability(task db.AgentTaskQueue) error {
	if !task.SessionID.Valid || strings.TrimSpace(task.SessionID.String) == "" {
		switch {
		case task.RetiredSessionID.Valid:
			return &AgentThreadUnavailableError{Reason: AgentThreadProviderSessionRetired}
		case task.SessionRolloutMissing:
			return &AgentThreadUnavailableError{Reason: AgentThreadProviderSessionMissing}
		case task.ForceFreshSession:
			return &AgentThreadUnavailableError{Reason: AgentThreadFreshSessionRequired}
		default:
			return &AgentThreadUnavailableError{Reason: AgentThreadProviderSessionNotEstablished}
		}
	}
	if task.RetiredSessionID.Valid && task.RetiredSessionID.String == task.SessionID.String {
		return &AgentThreadUnavailableError{Reason: AgentThreadProviderSessionRetired}
	}
	if task.SessionRolloutMissing {
		return &AgentThreadUnavailableError{Reason: AgentThreadProviderSessionMissing}
	}
	return nil
}

func AgentThreadBindingAvailability(task db.AgentTaskQueue, agent db.Agent, runtimeExists bool) error {
	switch {
	case agent.ArchivedAt.Valid:
		return &AgentThreadUnavailableError{Reason: AgentThreadAgentArchived}
	case !agent.RuntimeID.Valid:
		return &AgentThreadUnavailableError{Reason: AgentThreadAgentRuntimeUnbound}
	case agent.RuntimeID != task.RuntimeID:
		return &AgentThreadUnavailableError{Reason: AgentThreadAgentRuntimeRebound}
	case !runtimeExists:
		return &AgentThreadUnavailableError{Reason: AgentThreadAgentRuntimeMissing}
	default:
		return nil
	}
}

func normalizeAgentThreadInput(content, idempotencyKey string) (string, string, error) {
	content = strings.TrimSpace(util.SanitizeTextForPostgres(content))
	idempotencyKey = strings.TrimSpace(util.SanitizeTextForPostgres(idempotencyKey))
	if content == "" {
		return "", "", fmt.Errorf("agent thread message is empty")
	}
	if idempotencyKey == "" || len([]rune(idempotencyKey)) > 200 {
		return "", "", fmt.Errorf("agent thread idempotency key is invalid")
	}
	runes := []rune(content)
	if len(runes) > maxAgentThreadMessageRunes {
		content = string(runes[:maxAgentThreadMessageRunes])
	}
	return content, idempotencyKey, nil
}

func agentThreadInvocationAllowed(ctx context.Context, queries *db.Queries, agent db.Agent, requester pgtype.UUID) bool {
	if !requester.Valid {
		return false
	}
	if agent.OwnerID.Valid && agent.OwnerID == requester {
		return true
	}
	if agent.PermissionMode != "public_to" {
		return false
	}
	if _, err := queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
		UserID: requester, WorkspaceID: agent.WorkspaceID,
	}); err != nil {
		return false
	}
	targets, err := queries.ListAgentInvocationTargets(ctx, agent.ID)
	if err != nil {
		return false
	}
	for _, target := range targets {
		switch target.TargetType {
		case "workspace":
			return true
		case "member":
			if target.TargetID.Valid && target.TargetID == requester {
				return true
			}
		default:
			// Team and future target types do not grant member invocation here.
		}
	}
	return false
}

func (s *TaskService) ContinueAgentThread(ctx context.Context, parentTaskID pgtype.UUID, content, idempotencyKey string, requesterUserID pgtype.UUID) (AgentThreadContinuationReceipt, error) {
	content, idempotencyKey, err := normalizeAgentThreadInput(content, idempotencyKey)
	if err != nil {
		return AgentThreadContinuationReceipt{}, err
	}
	if s.TxStarter == nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("agent thread transaction unavailable")
	}

	snapshot, err := s.Queries.GetAgentTask(ctx, parentTaskID)
	if err != nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("load agent thread parent: %w", err)
	}
	if err := AgentThreadAvailability(snapshot); err != nil {
		return AgentThreadContinuationReceipt{}, err
	}
	agent, err := s.Queries.GetAgent(ctx, snapshot.AgentID)
	if err != nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("load agent thread agent: %w", err)
	}
	runtime, runtimeErr := s.Queries.GetAgentRuntime(ctx, snapshot.RuntimeID)
	if err := AgentThreadBindingAvailability(snapshot, agent, runtimeErr == nil && runtime.WorkspaceID == agent.WorkspaceID); err != nil {
		return AgentThreadContinuationReceipt{}, err
	}
	if !agentThreadInvocationAllowed(ctx, s.Queries, agent, requesterUserID) {
		return AgentThreadContinuationReceipt{}, ErrAgentThreadInvokeForbidden
	}
	overlay := s.buildRuntimeMCPOverlay(ctx, requesterUserID, agent)

	tx, err := s.TxStarter.Begin(ctx)
	if err != nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("begin agent thread continuation: %w", err)
	}
	defer tx.Rollback(ctx)
	qtx := s.Queries.WithTx(tx)

	parent, err := qtx.LockAgentThreadTask(ctx, parentTaskID)
	if err != nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("lock agent thread parent: %w", err)
	}
	if err := AgentThreadAvailability(parent); err != nil {
		return AgentThreadContinuationReceipt{}, err
	}
	lockedAgent, err := qtx.GetAgent(ctx, parent.AgentID)
	if err != nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("reload agent thread agent: %w", err)
	}
	runtime, runtimeErr = qtx.GetAgentRuntime(ctx, parent.RuntimeID)
	if err := AgentThreadBindingAvailability(parent, lockedAgent, runtimeErr == nil && runtime.WorkspaceID == lockedAgent.WorkspaceID); err != nil {
		return AgentThreadContinuationReceipt{}, err
	}
	if !agentThreadInvocationAllowed(ctx, qtx, lockedAgent, requesterUserID) {
		return AgentThreadContinuationReceipt{}, ErrAgentThreadInvokeForbidden
	}

	existing, err := qtx.GetAgentThreadContinuationByIdempotency(ctx, db.GetAgentThreadContinuationByIdempotencyParams{
		ParentTaskID:   parentTaskID,
		IdempotencyKey: idempotencyKey,
	})
	if err == nil {
		if AgentThreadMessage(existing) != content {
			return AgentThreadContinuationReceipt{}, ErrAgentThreadIdempotencyConflict
		}
		if err := tx.Commit(ctx); err != nil {
			return AgentThreadContinuationReceipt{}, err
		}
		return AgentThreadContinuationReceipt{Task: existing, Coalesced: true}, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("find agent thread continuation receipt: %w", err)
	}

	thread, err := qtx.ListAgentThreadTasks(ctx, parentTaskID)
	if err != nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("list agent thread tasks: %w", err)
	}
	if len(thread) >= maxAgentThreadDepth {
		return AgentThreadContinuationReceipt{}, ErrAgentThreadDepthLimit
	}

	continuation, err := qtx.CreateAgentThreadContinuation(ctx, db.CreateAgentThreadContinuationParams{
		ID:                   dbid.NewV7(),
		Content:              content,
		IdempotencyKey:       idempotencyKey,
		RequesterUserID:      requesterUserID,
		RuntimeMcpOverlay:    overlay.Overlay,
		RuntimeConnectedApps: overlay.ConnectedApps,
		ParentTaskID:         parentTaskID,
	})
	if err != nil {
		return AgentThreadContinuationReceipt{}, fmt.Errorf("create agent thread continuation: %w", err)
	}
	if err := tx.Commit(ctx); err != nil {
		return AgentThreadContinuationReceipt{}, err
	}

	s.broadcastTaskEvent(ctx, protocol.EventTaskQueued, continuation)
	s.NotifyTaskEnqueued(ctx, continuation)
	return AgentThreadContinuationReceipt{Task: continuation}, nil
}
