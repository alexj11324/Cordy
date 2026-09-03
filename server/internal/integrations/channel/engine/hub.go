package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/channel"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// HubResolution selects an invocable Agent for one workspace-owned channel
// conversation. Control replies never become Agent input.
type HubResolution struct {
	AgentID       pgtype.UUID
	ReplyText     string
	Handled       bool
	EnsureSession bool
	SwitchesAgent bool
}

type HubPersistParams struct {
	Installation ResolvedInstallation
	UserID       pgtype.UUID
	BindingKey   string
	SessionID    pgtype.UUID
	AgentID      pgtype.UUID
	// /new persists the selection inside its existing session/task transaction.
	// Ordinary messages and /agents leave Tx nil and get an owned transaction.
	Tx pgx.Tx
}

type HubRouter interface {
	Resolve(context.Context, ResolvedInstallation, ResolvedIdentity, channel.InboundMessage, string) (HubResolution, error)
	PersistRoute(context.Context, HubPersistParams) error
}

type hubReadQueries interface {
	ListAgents(context.Context, pgtype.UUID) ([]db.Agent, error)
	ListAgentInvocationTargetsByAgentIDs(context.Context, []pgtype.UUID) ([]db.AgentInvocationTarget, error)
	GetChannelHubRoute(context.Context, db.GetChannelHubRouteParams) ([]byte, error)
	GetUser(context.Context, pgtype.UUID) (db.User, error)
}

// PostgresHubRouter is shared by all channel adapters. It does not equate
// workspace administrator visibility with permission to invoke an Agent.
type PostgresHubRouter struct {
	q  hubReadQueries
	tx TxStarter
}

func NewPostgresHubRouter(q *db.Queries, tx TxStarter) *PostgresHubRouter {
	return &PostgresHubRouter{q: q, tx: tx}
}

func (h *PostgresHubRouter) availableAgents(ctx context.Context, workspaceID, userID pgtype.UUID) ([]db.Agent, error) {
	agents, err := h.q.ListAgents(ctx, workspaceID)
	if err != nil {
		return nil, err
	}
	ids := make([]pgtype.UUID, 0, len(agents))
	for _, agent := range agents {
		ids = append(ids, agent.ID)
	}
	targets, err := h.q.ListAgentInvocationTargetsByAgentIDs(ctx, ids)
	if err != nil {
		return nil, err
	}
	allowed := make(map[pgtype.UUID]bool)
	for _, target := range targets {
		if target.TargetType == "workspace" || (target.TargetType == "member" && target.TargetID == userID) {
			allowed[target.AgentID] = true
		}
	}
	available := make([]db.Agent, 0, len(agents))
	for _, agent := range agents {
		if agent.WorkspaceID != workspaceID || agent.ArchivedAt.Valid || agent.Kind != "user" {
			continue
		}
		if (agent.OwnerID.Valid && agent.OwnerID == userID) || (agent.PermissionMode == "public_to" && allowed[agent.ID]) {
			available = append(available, agent)
		}
	}
	return available, nil
}

func (h *PostgresHubRouter) currentAgent(ctx context.Context, installationID pgtype.UUID, bindingKey string) (pgtype.UUID, error) {
	channelID, _, _ := strings.Cut(bindingKey, ":")
	raw, err := h.q.GetChannelHubRoute(ctx, db.GetChannelHubRouteParams{
		InstallationID: installationID, BindingKey: bindingKey, ChannelID: channelID,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return pgtype.UUID{}, nil
	}
	if err != nil {
		return pgtype.UUID{}, err
	}
	var config map[string]json.RawMessage
	if err := json.Unmarshal(raw, &config); err != nil {
		return pgtype.UUID{}, err
	}
	var id string
	if json.Unmarshal(config["hub_agent_id"], &id) != nil {
		return pgtype.UUID{}, nil
	}
	var parsed pgtype.UUID
	if parsed.Scan(id) != nil {
		return pgtype.UUID{}, nil
	}
	return parsed, nil
}

func (h *PostgresHubRouter) Resolve(ctx context.Context, inst ResolvedInstallation, identity ResolvedIdentity, msg channel.InboundMessage, bindingKey string) (HubResolution, error) {
	agents, err := h.availableAgents(ctx, inst.WorkspaceID, identity.UserID)
	if err != nil {
		return HubResolution{}, err
	}
	current, err := h.currentAgent(ctx, inst.ID, bindingKey)
	if err != nil {
		return HubResolution{}, err
	}
	var selected *db.Agent
	for i := range agents {
		if selected == nil || agents[i].ID == current {
			selected = &agents[i]
		}
	}
	result := HubResolution{}
	if selected != nil {
		result.AgentID = selected.ID
		result.SwitchesAgent = current.Valid && current != selected.ID
	}
	selector, command := parseAgentsCommand(msg.CommandText)
	if !command && selected != nil {
		return result, nil
	}
	user, err := h.q.GetUser(ctx, identity.UserID)
	if err != nil {
		return HubResolution{}, err
	}
	copy := channel.HubCopyForLocale(user.Language.String)
	result.Handled = true
	if !command {
		result.ReplyText = copy.NoAvailable
		return result, nil
	}
	if selector == "" {
		result.ReplyText = renderHubAgents(agents, result.AgentID, copy)
		return result, nil
	}
	selected = selectHubAgent(agents, selector)
	if selected == nil {
		result.ReplyText = fmt.Sprintf(copy.NotFound, hubDisplayText(selector)) + "\n\n" + renderHubAgents(agents, result.AgentID, copy)
		return result, nil
	}
	result.AgentID = selected.ID
	result.EnsureSession, result.SwitchesAgent = true, true
	result.ReplyText = fmt.Sprintf(copy.Switched, hubDisplayText(selected.Name))
	return result, nil
}

var ErrHubAgentUnavailable = errors.New("selected Hub Agent is no longer invocable")

func (h *PostgresHubRouter) PersistRoute(ctx context.Context, p HubPersistParams) error {
	if p.Tx != nil {
		return persistHubRoute(ctx, db.New(p.Tx), p)
	}
	if h.tx == nil {
		return errors.New("channel Hub transaction starter is unavailable")
	}
	tx, err := h.tx.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx)
	if err := persistHubRoute(ctx, db.New(tx), p); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func persistHubRoute(ctx context.Context, q *db.Queries, p HubPersistParams) error {
	if _, err := q.LockWorkspaceForChatSessionCreate(ctx, p.Installation.WorkspaceID); err != nil {
		return err
	}
	if _, err := q.LockChannelInstallationForHub(ctx, db.LockChannelInstallationForHubParams{
		InstallationID: p.Installation.ID, WorkspaceID: p.Installation.WorkspaceID,
	}); err != nil {
		return err
	}
	// Match append/enqueue's Chat -> binding order. The Agent and both resume
	// pointers change with the binding selection, or none of them change.
	if _, err := q.LockChatSessionForRuntimeBind(ctx, p.SessionID); err != nil {
		return err
	}
	count, err := q.SwitchHubChatSessionAgent(ctx, db.SwitchHubChatSessionAgentParams{
		ChatSessionID: p.SessionID, WorkspaceID: p.Installation.WorkspaceID, AgentID: p.AgentID, UserID: p.UserID,
	})
	if err != nil {
		return err
	}
	if count != 1 {
		return ErrHubAgentUnavailable
	}
	count, err = q.MergeChannelHubRoute(ctx, db.MergeChannelHubRouteParams{
		InstallationID: p.Installation.ID, BindingKey: p.BindingKey, ChatSessionID: p.SessionID, AgentID: p.AgentID,
	})
	if err != nil {
		return err
	}
	if count != 1 {
		return ErrRouteChanged
	}
	return nil
}

func parseAgentsCommand(text string) (string, bool) {
	words := strings.Fields(text)
	if len(words) == 0 {
		return "", false
	}
	command, _, _ := strings.Cut(strings.ToLower(words[0]), "@")
	if command != "/agents" && command != "/agent" {
		return "", false
	}
	return strings.Join(words[1:], " "), true
}

func selectHubAgent(agents []db.Agent, selector string) *db.Agent {
	if index, err := strconv.Atoi(selector); err == nil && index >= 0 {
		if index > 0 && index <= len(agents) {
			return &agents[index-1]
		}
		return nil
	}
	for i := range agents {
		if util.UUIDToString(agents[i].ID) == selector || strings.EqualFold(agents[i].Name, selector) {
			return &agents[i]
		}
	}
	return nil
}

func renderHubAgents(agents []db.Agent, current pgtype.UUID, copy channel.HubCopy) string {
	if len(agents) == 0 {
		return copy.Empty
	}
	var text strings.Builder
	text.WriteString(copy.Available + "\n")
	for i, agent := range agents {
		marker := ""
		if agent.ID == current {
			marker = " " + copy.Current
		}
		fmt.Fprintf(&text, "%d. %s%s\n", i+1, hubDisplayText(agent.Name), marker)
	}
	text.WriteString("\n" + copy.SwitchHelp)
	return text.String()
}

func hubDisplayText(text string) string {
	text = strings.Join(strings.Fields(text), " ")
	text = channel.BreakMarkdownLinkAdjacency(text)
	return strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;", "\\", "\\\\", "*", "\\*", "_", "\\_", "`", "\\`", "[", "\\[", "]", "\\]").Replace(text)
}
