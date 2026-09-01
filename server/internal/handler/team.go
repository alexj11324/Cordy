package handler

import (
	"context"
	"encoding/json"
	"log/slog"
	"net/http"
	"strconv"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/analytics"
	"github.com/patchbay-ai/patchbay/server/internal/logger"
	obsmetrics "github.com/patchbay-ai/patchbay/server/internal/metrics"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// ── Response types ──────────────────────────────────────────────────────────

type TeamResponse struct {
	ID            string                       `json:"id"`
	WorkspaceID   string                       `json:"workspace_id"`
	Name          string                       `json:"name"`
	Description   string                       `json:"description"`
	Instructions  string                       `json:"instructions"`
	AvatarURL     *string                      `json:"avatar_url"`
	LeaderID      string                       `json:"leader_id"`
	CreatorID     string                       `json:"creator_id"`
	CreatedAt     string                       `json:"created_at"`
	UpdatedAt     string                       `json:"updated_at"`
	ArchivedAt    *string                      `json:"archived_at"`
	ArchivedBy    *string                      `json:"archived_by"`
	MemberCount   int                          `json:"member_count"`
	MemberPreview []TeamMemberPreviewResponse `json:"member_preview"`
}

type TeamMemberPreviewResponse struct {
	MemberType string `json:"member_type"`
	MemberID   string `json:"member_id"`
	Role       string `json:"role"`
}

type teamMemberSummary struct {
	count   int
	preview []TeamMemberPreviewResponse
}

type TeamMemberResponse struct {
	ID         string `json:"id"`
	TeamID    string `json:"team_id"`
	MemberType string `json:"member_type"`
	MemberID   string `json:"member_id"`
	Role       string `json:"role"`
	CreatedAt  string `json:"created_at"`
}

// ── Converters ──────────────────────────────────────────────────────────────

func (h *Handler) teamToResponse(s db.Team) TeamResponse {
	return TeamResponse{
		ID:            uuidToString(s.ID),
		WorkspaceID:   uuidToString(s.WorkspaceID),
		Name:          s.Name,
		Description:   s.Description,
		Instructions:  s.Instructions,
		AvatarURL:     h.resolveAvatarURLPtr(textToPtr(s.AvatarUrl)),
		LeaderID:      uuidToString(s.LeaderID),
		CreatorID:     uuidToString(s.CreatorID),
		CreatedAt:     timestampToString(s.CreatedAt),
		UpdatedAt:     timestampToString(s.UpdatedAt),
		ArchivedAt:    timestampToPtr(s.ArchivedAt),
		ArchivedBy:    uuidToPtr(s.ArchivedBy),
		MemberPreview: []TeamMemberPreviewResponse{},
	}
}

func teamMemberToResponse(m db.TeamMember) TeamMemberResponse {
	return TeamMemberResponse{
		ID:         uuidToString(m.ID),
		TeamID:    uuidToString(m.TeamID),
		MemberType: m.MemberType,
		MemberID:   uuidToString(m.MemberID),
		Role:       m.Role,
		CreatedAt:  timestampToString(m.CreatedAt),
	}
}

func addTeamMemberPreview(summary *teamMemberSummary, memberType string, memberID pgtype.UUID, role string) {
	summary.count++
	if len(summary.preview) >= 3 {
		return
	}
	summary.preview = append(summary.preview, TeamMemberPreviewResponse{
		MemberType: memberType,
		MemberID:   uuidToString(memberID),
		Role:       role,
	})
}

func applyTeamMemberSummary(resp *TeamResponse, summary *teamMemberSummary) {
	if summary == nil {
		return
	}
	resp.MemberCount = summary.count
	resp.MemberPreview = summary.preview
}

// ── Helpers ─────────────────────────────────────────────────────────────────

// canManageTeam reports whether the member may mutate the team. Workspace
// owner/admin manage every team; a regular member manages only the teams
// they created. Teams stay creator-scoped for management while remaining
// visible workspace-wide (ListTeams is unfiltered). Mirrors the front-end
// per-team `canManage` gate so the UI and API agree on who can rename / add
// members / archive (MUL-4223).
func canManageTeam(member db.Member, team db.Team) bool {
	if roleAllowed(member.Role, "owner", "admin") {
		return true
	}
	return uuidToString(team.CreatorID) == uuidToString(member.UserID)
}

// memberCanWireAgent reports whether the acting member may attach the given
// agent to a team (as leader or worker). Workspace owner/admin may wire any
// workspace agent — their management surface is unchanged. A regular member
// (a creator managing their own team) may only wire agents they can
// @-trigger: canInvokeAgent judged as the member themselves, so public_to
// agents on their allow-list and their own private agents pass, while other
// members' private / non-allow-listed agents are rejected. This stops a
// creator from smuggling an agent they cannot invoke into a team and reaching
// it through team routing (MUL-4223).
func (h *Handler) memberCanWireAgent(ctx context.Context, member db.Member, agent db.Agent, workspaceID string) bool {
	if roleAllowed(member.Role, "owner", "admin") {
		return true
	}
	uid := uuidToString(member.UserID)
	return h.canInvokeAgent(ctx, agent, "member", uid, uid, workspaceID)
}

// loadTeamInWorkspace loads a team scoped to the current workspace.
func (h *Handler) loadTeamInWorkspace(w http.ResponseWriter, r *http.Request) (db.Team, string, bool) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	teamID := chi.URLParam(r, "id")
	teamUUID, ok := parseUUIDOrBadRequest(w, teamID, "team id")
	if !ok {
		return db.Team{}, "", false
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace_id")
	if !ok {
		return db.Team{}, "", false
	}
	team, err := h.Queries.GetTeamInWorkspace(r.Context(), db.GetTeamInWorkspaceParams{
		ID:          teamUUID,
		WorkspaceID: wsUUID,
	})
	if err != nil {
		writeError(w, http.StatusNotFound, "team not found")
		return db.Team{}, "", false
	}
	return team, workspaceID, true
}

func (h *Handler) loadTeamMemberSummary(ctx context.Context, teamID pgtype.UUID) (*teamMemberSummary, error) {
	rows, err := h.Queries.ListTeamMemberPreviewRowsByTeam(ctx, teamID)
	if err != nil {
		return nil, err
	}
	summary := &teamMemberSummary{}
	for _, row := range rows {
		addTeamMemberPreview(summary, row.MemberType, row.MemberID, row.Role)
	}
	return summary, nil
}

func (h *Handler) teamToResponseWithPreview(ctx context.Context, team db.Team) (TeamResponse, error) {
	resp := h.teamToResponse(team)
	summary, err := h.loadTeamMemberSummary(ctx, team.ID)
	if err != nil {
		return resp, err
	}
	applyTeamMemberSummary(&resp, summary)
	return resp, nil
}

// ── Handlers ────────────────────────────────────────────────────────────────

func (h *Handler) ListTeams(w http.ResponseWriter, r *http.Request) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace_id")
	if !ok {
		return
	}
	teams, err := h.Queries.ListTeams(r.Context(), wsUUID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list teams")
		return
	}

	previewRows, err := h.Queries.ListTeamMemberPreviewRows(r.Context(), wsUUID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list team member preview")
		return
	}
	summaries := make(map[string]*teamMemberSummary, len(teams))
	for _, row := range previewRows {
		teamID := uuidToString(row.TeamID)
		summary := summaries[teamID]
		if summary == nil {
			summary = &teamMemberSummary{}
			summaries[teamID] = summary
		}
		addTeamMemberPreview(summary, row.MemberType, row.MemberID, row.Role)
	}

	resp := make([]TeamResponse, len(teams))
	for i, s := range teams {
		resp[i] = h.teamToResponse(s)
		applyTeamMemberSummary(&resp[i], summaries[uuidToString(s.ID)])
	}
	writeJSON(w, http.StatusOK, resp)
}

func (h *Handler) CreateTeam(w http.ResponseWriter, r *http.Request) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	// Any workspace member can create a team and becomes its creator
	// (CreatorID below). This aligns teams with agents/projects, which are
	// also member-creatable; management stays creator-scoped (MUL-4223).
	member, ok := h.requireWorkspaceMember(w, r, workspaceID, "workspace not found")
	if !ok {
		return
	}

	var req struct {
		Name        string  `json:"name"`
		Description string  `json:"description"`
		LeaderID    string  `json:"leader_id"`
		AvatarURL   *string `json:"avatar_url"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.Name == "" {
		writeError(w, http.StatusBadRequest, "name is required")
		return
	}
	if req.LeaderID == "" {
		writeError(w, http.StatusBadRequest, "leader_id is required")
		return
	}

	leaderUUID, ok := parseUUIDOrBadRequest(w, req.LeaderID, "leader_id")
	if !ok {
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace_id")
	if !ok {
		return
	}

	// Validate leader is an agent in this workspace.
	leaderAgent, err := h.Queries.GetAgentInWorkspace(r.Context(), db.GetAgentInWorkspaceParams{
		ID:          leaderUUID,
		WorkspaceID: wsUUID,
	})
	if err != nil {
		writeError(w, http.StatusBadRequest, "leader must be a valid agent in this workspace")
		return
	}
	// A non-admin creator may only lead their team with an agent they can
	// @-trigger; admins may wire any workspace agent (MUL-4223).
	if !h.memberCanWireAgent(r.Context(), member, leaderAgent, workspaceID) {
		writeError(w, http.StatusForbidden, "you can only use an agent you have access to as leader")
		return
	}

	avatarURL := pgtype.Text{}
	if req.AvatarURL != nil {
		accepted, ok := h.acceptAvatarURL(w, r, *req.AvatarURL, "")
		if !ok {
			return
		}
		avatarURL = pgtype.Text{String: accepted, Valid: true}
	}

	team, err := h.Queries.CreateTeam(r.Context(), db.CreateTeamParams{
		WorkspaceID: wsUUID,
		Name:        req.Name,
		Description: req.Description,
		LeaderID:    leaderUUID,
		CreatorID:   member.UserID,
		AvatarUrl:   avatarURL,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create team")
		return
	}

	// Auto-add leader as a member with role "leader".
	h.Queries.AddTeamMember(r.Context(), db.AddTeamMemberParams{
		TeamID:    team.ID,
		MemberType: "agent",
		MemberID:   leaderUUID,
		Role:       "leader",
	})

	resp, err := h.teamToResponseWithPreview(r.Context(), team)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load team member preview")
		return
	}
	h.publish(protocol.EventTeamCreated, workspaceID, "member", uuidToString(member.UserID), map[string]any{"team": resp})
	obsmetrics.RecordEvent(h.Analytics, h.Metrics, analytics.TeamCreated(
		uuidToString(member.UserID),
		workspaceID,
		uuidToString(team.ID),
		1,
	))
	writeJSON(w, http.StatusCreated, resp)
}

func (h *Handler) GetTeam(w http.ResponseWriter, r *http.Request) {
	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}
	resp, err := h.teamToResponseWithPreview(r.Context(), team)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load team member preview")
		return
	}
	writeJSON(w, http.StatusOK, resp)
}

func (h *Handler) UpdateTeam(w http.ResponseWriter, r *http.Request) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	member, ok := h.requireWorkspaceMember(w, r, workspaceID, "workspace not found")
	if !ok {
		return
	}

	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}
	if !canManageTeam(member, team) {
		writeError(w, http.StatusForbidden, "insufficient permissions")
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace_id")
	if !ok {
		return
	}

	var req struct {
		Name         *string `json:"name"`
		Description  *string `json:"description"`
		Instructions *string `json:"instructions"`
		LeaderID     *string `json:"leader_id"`
		AvatarURL    *string `json:"avatar_url"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	params := db.UpdateTeamParams{ID: team.ID}
	if req.Name != nil {
		params.Name = pgtype.Text{String: *req.Name, Valid: true}
	}
	if req.Description != nil {
		params.Description = pgtype.Text{String: *req.Description, Valid: true}
	}
	if req.Instructions != nil {
		params.Instructions = pgtype.Text{String: *req.Instructions, Valid: true}
	}
	if req.AvatarURL != nil {
		accepted, ok := h.acceptAvatarURL(w, r, *req.AvatarURL, team.AvatarUrl.String)
		if !ok {
			return
		}
		params.AvatarUrl = pgtype.Text{String: accepted, Valid: true}
	}

	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update team")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	// Automation assignment takes FOR SHARE on the team before locking its
	// leader Agent. Take the exclusive side in the same order so leader
	// rotation and active Automation saves cannot leave an automation pointing
	// at an unbound effective Agent.
	if _, err := qtx.LockTeamForUpdate(r.Context(), db.LockTeamForUpdateParams{
		ID:          team.ID,
		WorkspaceID: wsUUID,
	}); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update team")
		return
	}

	newLeaderRuntimeBound := true
	if req.LeaderID != nil {
		lid, ok := parseUUIDOrBadRequest(w, *req.LeaderID, "leader_id")
		if !ok {
			return
		}
		// Stabilize runtime_id through commit. Runtime teardown takes FOR UPDATE
		// on this row and follows the same Agent→Automation lock order, so
		// whichever operation starts first produces a complete result.
		newLeader, err := qtx.LockAgentForAutomationAssignment(r.Context(), db.LockAgentForAutomationAssignmentParams{
			ID:          lid,
			WorkspaceID: wsUUID,
		})
		if err != nil {
			writeError(w, http.StatusBadRequest, "leader must be a valid agent in this workspace")
			return
		}
		// A non-admin creator may only promote an agent they can @-trigger.
		if !h.memberCanWireAgent(r.Context(), member, newLeader, workspaceID) {
			writeError(w, http.StatusForbidden, "you can only use an agent you have access to as leader")
			return
		}
		// Ensure new leader is a team member; auto-add if not.
		isMember, err := qtx.IsTeamMember(r.Context(), db.IsTeamMemberParams{
			TeamID: team.ID, MemberType: "agent", MemberID: lid,
		})
		if err != nil {
			writeError(w, http.StatusInternalServerError, "failed to update team")
			return
		}
		if !isMember {
			if _, err := qtx.AddTeamMember(r.Context(), db.AddTeamMemberParams{
				TeamID: team.ID, MemberType: "agent", MemberID: lid, Role: "leader",
			}); err != nil {
				writeError(w, http.StatusInternalServerError, "failed to update team")
				return
			}
		}
		params.LeaderID = lid
		newLeaderRuntimeBound = newLeader.RuntimeID.Valid
	}

	updated, err := qtx.UpdateTeam(r.Context(), params)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update team")
		return
	}
	var pausedAutomations []db.Automation
	if req.LeaderID != nil && !newLeaderRuntimeBound {
		pausedAutomations, err = qtx.PauseAutomationsByUnrunnableTeam(r.Context(), team.ID)
		if err != nil {
			writeError(w, http.StatusInternalServerError, "failed to update team")
			return
		}
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update team")
		return
	}

	resp, err := h.teamToResponseWithPreview(r.Context(), updated)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load team member preview")
		return
	}
	h.publish(protocol.EventTeamUpdated, workspaceID, "member", requestUserID(r), map[string]any{"team": resp})
	for _, automation := range pausedAutomations {
		h.publish(protocol.EventAutomationUpdated, workspaceID, "member", requestUserID(r), map[string]any{
			"automation": automationToResponse(automation, nil),
		})
	}
	writeJSON(w, http.StatusOK, resp)
}

func (h *Handler) DeleteTeam(w http.ResponseWriter, r *http.Request) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	member, ok := h.requireWorkspaceMember(w, r, workspaceID, "workspace not found")
	if !ok {
		return
	}

	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}
	if !canManageTeam(member, team) {
		writeError(w, http.StatusForbidden, "insufficient permissions")
		return
	}

	if team.ArchivedAt.Valid {
		writeError(w, http.StatusBadRequest, "team is already archived")
		return
	}

	// Transfer issues assigned to this team to the leader agent.
	if err := h.Queries.TransferTeamAssignees(r.Context(), db.TransferTeamAssigneesParams{
		ExecutorID:   team.ID,
		ExecutorID_2: team.LeaderID,
	}); err != nil {
		slog.Warn("transfer team assignees failed", "team_id", uuidToString(team.ID), "error", err)
	}

	// Mirror the issue-assignee transfer for automations that target this
	// team. Without this, automation.executor_id would still point at the
	// archived team row and every subsequent dispatch would skip with
	// "assignee team is archived" — visible to ops but useless to the
	// owner. Rewriting to the leader keeps the automation semantics
	// unchanged (Path A from MUL-2429 is leader-only execution anyway).
	if err := h.Queries.TransferTeamAutomationsToLeader(r.Context(), db.TransferTeamAutomationsToLeaderParams{
		ExecutorID:   team.ID,
		ExecutorID_2: team.LeaderID,
	}); err != nil {
		slog.Warn("transfer team automations failed", "team_id", uuidToString(team.ID), "error", err)
	}

	userID := requestUserID(r)
	userUUID, _ := parseUUIDOrBadRequest(w, userID, "user_id")

	if _, err := h.Queries.ArchiveTeam(r.Context(), db.ArchiveTeamParams{
		ID:         team.ID,
		ArchivedBy: userUUID,
	}); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to archive team")
		return
	}

	h.publish(protocol.EventTeamDeleted, workspaceID, "member", userID, map[string]any{
		"team_id":  uuidToString(team.ID),
		"leader_id": uuidToString(team.LeaderID),
	})
	w.WriteHeader(http.StatusNoContent)
}

// ── Team Members ───────────────────────────────────────────────────────────

func (h *Handler) ListTeamMembers(w http.ResponseWriter, r *http.Request) {
	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}
	members, err := h.Queries.ListTeamMembers(r.Context(), team.ID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list team members")
		return
	}
	resp := make([]TeamMemberResponse, len(members))
	for i, m := range members {
		resp[i] = teamMemberToResponse(m)
	}
	writeJSON(w, http.StatusOK, resp)
}

// ── Team Member Status ────────────────────────────────────────────────────

// TeamMemberStatus is the per-member entry in the team member status
// response. Agent members carry a derived working/idle/offline/unstable
// status plus any active issues; human members are returned with member_type
// only so the front-end can render them in the same list without
// reordering.
type TeamMemberStatusResponse struct {
	MemberType   string                  `json:"member_type"`
	MemberID     string                  `json:"member_id"`
	Status       *string                 `json:"status"`
	ActiveIssues []TeamActiveIssueBrief `json:"active_issues"`
	LastActiveAt *string                 `json:"last_active_at"`
}

type TeamActiveIssueBrief struct {
	IssueID     string `json:"issue_id"`
	Identifier  string `json:"identifier"`
	Title       string `json:"title"`
	IssueStatus string `json:"issue_status"`
}

type TeamMemberStatusListResponse struct {
	Members []TeamMemberStatusResponse `json:"members"`
}

// deriveTeamMemberStatus collapses runtime + task signals into the five
// status buckets used by the team UI. Mirrors the workload+availability
// split in packages/core/agents/derive-presence.ts: working wins over
// runtime health (an agent that is in the middle of dispatched/running
// work counts as working even if the runtime briefly drops), then
// availability buckets decide between idle / unstable / offline.
//
// Thresholds match deriveRuntimeHealth: any offline runtime whose
// last_seen_at is within the last 5 minutes is reported as "unstable" so
// the team UI surfaces transient drops the same way the agent dot does.
//
// Archived agents always report `archived` regardless of any leftover
// runtime row or task — they should appear in the list but never look
// like they're still working or merely offline (a leftover online
// runtime row would otherwise read as "offline" and hide the fact that
// the agent has been archived). Per the RFC decision (see MUL-2319), we
// surface archived agents in this endpoint rather than filtering them
// out in the SQL.
func deriveTeamMemberStatus(
	archived bool,
	runtimeStatus pgtype.Text,
	lastSeen pgtype.Timestamptz,
	hasWorkingTask bool,
	now time.Time,
) string {
	if archived {
		return "archived"
	}
	if hasWorkingTask {
		return "working"
	}
	if !runtimeStatus.Valid {
		return "offline"
	}
	if runtimeStatus.String == "online" {
		return "idle"
	}
	if !lastSeen.Valid {
		return "offline"
	}
	if now.Sub(lastSeen.Time) < 5*time.Minute {
		return "unstable"
	}
	return "offline"
}

// ListTeamMemberStatus returns one entry per team member with derived
// status, the issues each agent member is currently running or waiting to run,
// and the last observed runtime activity. The endpoint is read-only and
// inherits the workspace-membership guard from the route middleware — any
// member of the workspace can read it.
func (h *Handler) ListTeamMemberStatus(w http.ResponseWriter, r *http.Request) {
	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}

	rows, err := h.Queries.ListTeamMemberStatusRows(r.Context(), team.ID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list team member status")
		return
	}

	prefix := h.getIssuePrefix(r.Context(), team.WorkspaceID)
	now := time.Now()

	// Group rows by member_id while preserving the SQL ORDER BY (team_member
	// insertion order). One member may appear in multiple rows when they have
	// more than one active task.
	type memberAcc struct {
		response       TeamMemberStatusResponse
		archived       bool
		hasWorkingTask bool
		runtimeStatus  pgtype.Text
		runtimeSeenAt  pgtype.Timestamptz
		latestActiveAt pgtype.Timestamptz
	}
	order := make([]string, 0, len(rows))
	acc := make(map[string]*memberAcc, len(rows))

	for _, row := range rows {
		memberID := uuidToString(row.MemberID)
		entry, exists := acc[memberID]
		if !exists {
			entry = &memberAcc{
				response: TeamMemberStatusResponse{
					MemberType:   row.MemberType,
					MemberID:     memberID,
					ActiveIssues: []TeamActiveIssueBrief{},
				},
				archived:      row.AgentArchivedAt.Valid,
				runtimeStatus: row.RuntimeStatus,
				runtimeSeenAt: row.RuntimeLastSeenAt,
			}
			acc[memberID] = entry
			order = append(order, memberID)
		}

		if row.MemberType != "agent" {
			continue
		}

		// Keep waiting_local_directory rows available for issue visibility,
		// but only dispatched/running work drives the `working` bucket. A
		// working task may have no issue (chat / quick-create), so decide the
		// bucket independently from whether an issue link can be rendered.
		if row.TaskID.Valid {
			if row.TaskStatus.Valid &&
				(row.TaskStatus.String == "dispatched" || row.TaskStatus.String == "running") {
				entry.hasWorkingTask = true
			}

			if row.TaskIssueID.Valid {
				brief := TeamActiveIssueBrief{
					IssueID:    uuidToString(row.TaskIssueID),
					Identifier: prefix + "-" + strconv.Itoa(int(row.IssueNumber.Int32)),
					Title:      row.IssueTitle.String,
					IssueStatus: func() string {
						if row.IssueStatus.Valid {
							return row.IssueStatus.String
						}
						return ""
					}(),
				}
				entry.response.ActiveIssues = append(entry.response.ActiveIssues, brief)
			}

			if row.TaskDispatchedAt.Valid && (!entry.latestActiveAt.Valid ||
				row.TaskDispatchedAt.Time.After(entry.latestActiveAt.Time)) {
				entry.latestActiveAt = row.TaskDispatchedAt
			}
		}
	}

	resp := TeamMemberStatusListResponse{
		Members: make([]TeamMemberStatusResponse, 0, len(order)),
	}
	for _, id := range order {
		entry := acc[id]
		if entry.response.MemberType == "agent" {
			status := deriveTeamMemberStatus(
				entry.archived,
				entry.runtimeStatus,
				entry.runtimeSeenAt,
				entry.hasWorkingTask,
				now,
			)
			entry.response.Status = &status
			// last_active_at prefers the freshest active-task dispatch
			// over the runtime heartbeat: a working agent should not
			// look stale because the runtime heartbeat is a few seconds
			// behind. Falls back to runtime last_seen_at otherwise.
			if entry.latestActiveAt.Valid {
				entry.response.LastActiveAt = timestampToPtr(entry.latestActiveAt)
			} else if entry.runtimeSeenAt.Valid {
				entry.response.LastActiveAt = timestampToPtr(entry.runtimeSeenAt)
			}
		}
		resp.Members = append(resp.Members, entry.response)
	}

	writeJSON(w, http.StatusOK, resp)
}

func (h *Handler) AddTeamMember(w http.ResponseWriter, r *http.Request) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	member, ok := h.requireWorkspaceMember(w, r, workspaceID, "workspace not found")
	if !ok {
		return
	}

	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}
	if !canManageTeam(member, team) {
		writeError(w, http.StatusForbidden, "insufficient permissions")
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace_id")
	if !ok {
		return
	}

	var req struct {
		MemberType string `json:"member_type"`
		MemberID   string `json:"member_id"`
		Role       string `json:"role"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.MemberType != "agent" && req.MemberType != "member" {
		writeError(w, http.StatusBadRequest, "member_type must be 'agent' or 'member'")
		return
	}
	if req.MemberID == "" {
		writeError(w, http.StatusBadRequest, "member_id is required")
		return
	}

	memberUUID, ok := parseUUIDOrBadRequest(w, req.MemberID, "member_id")
	if !ok {
		return
	}

	// Validate the member belongs to this workspace.
	if req.MemberType == "agent" {
		agent, err := h.Queries.GetAgentInWorkspace(r.Context(), db.GetAgentInWorkspaceParams{
			ID: memberUUID, WorkspaceID: wsUUID,
		})
		if err != nil {
			writeError(w, http.StatusBadRequest, "agent not found in this workspace")
			return
		}
		// A non-admin creator may only add agents they can @-trigger (public
		// or their own / allow-listed agents); admins may add any workspace
		// agent (MUL-4223).
		if !h.memberCanWireAgent(r.Context(), member, agent, workspaceID) {
			writeError(w, http.StatusForbidden, "you can only add an agent you have access to")
			return
		}
	} else {
		if _, err := h.Queries.GetMemberByUserAndWorkspace(r.Context(), db.GetMemberByUserAndWorkspaceParams{
			UserID: memberUUID, WorkspaceID: wsUUID,
		}); err != nil {
			writeError(w, http.StatusBadRequest, "member not found in this workspace")
			return
		}
	}

	sm, err := h.Queries.AddTeamMember(r.Context(), db.AddTeamMemberParams{
		TeamID:    team.ID,
		MemberType: req.MemberType,
		MemberID:   memberUUID,
		Role:       req.Role,
	})
	if err != nil {
		if isUniqueViolation(err) {
			writeError(w, http.StatusConflict, "member already in team")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to add team member")
		return
	}

	writeJSON(w, http.StatusCreated, teamMemberToResponse(sm))
	h.publish(protocol.EventTeamUpdated, workspaceID, "member", requestUserID(r), map[string]any{
		"team_id": uuidToString(team.ID),
	})
}

func (h *Handler) RemoveTeamMember(w http.ResponseWriter, r *http.Request) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	member, ok := h.requireWorkspaceMember(w, r, workspaceID, "workspace not found")
	if !ok {
		return
	}

	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}
	if !canManageTeam(member, team) {
		writeError(w, http.StatusForbidden, "insufficient permissions")
		return
	}

	var req struct {
		MemberType string `json:"member_type"`
		MemberID   string `json:"member_id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	memberUUID, ok := parseUUIDOrBadRequest(w, req.MemberID, "member_id")
	if !ok {
		return
	}

	// Prevent removing the leader.
	if req.MemberType == "agent" && uuidToString(team.LeaderID) == req.MemberID {
		writeError(w, http.StatusBadRequest, "cannot remove the team leader; change leader first")
		return
	}

	rows, err := h.Queries.RemoveTeamMember(r.Context(), db.RemoveTeamMemberParams{
		TeamID:    team.ID,
		MemberType: req.MemberType,
		MemberID:   memberUUID,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to remove team member")
		return
	}
	if rows == 0 {
		writeError(w, http.StatusNotFound, "team member not found")
		return
	}

	h.publish(protocol.EventTeamUpdated, workspaceID, "member", requestUserID(r), map[string]any{
		"team_id": uuidToString(team.ID),
	})
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) UpdateTeamMemberRole(w http.ResponseWriter, r *http.Request) {
	workspaceID := workspaceIDFromURL(r, "workspaceId")
	member, ok := h.requireWorkspaceMember(w, r, workspaceID, "workspace not found")
	if !ok {
		return
	}

	team, _, ok := h.loadTeamInWorkspace(w, r)
	if !ok {
		return
	}
	if !canManageTeam(member, team) {
		writeError(w, http.StatusForbidden, "insufficient permissions")
		return
	}

	var req struct {
		MemberType string `json:"member_type"`
		MemberID   string `json:"member_id"`
		Role       string `json:"role"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	memberUUID, ok := parseUUIDOrBadRequest(w, req.MemberID, "member_id")
	if !ok {
		return
	}

	sm, err := h.Queries.UpdateTeamMemberRole(r.Context(), db.UpdateTeamMemberRoleParams{
		TeamID:    team.ID,
		MemberType: req.MemberType,
		MemberID:   memberUUID,
		Role:       req.Role,
	})
	if err != nil {
		writeError(w, http.StatusNotFound, "team member not found")
		return
	}

	h.publish(protocol.EventTeamUpdated, workspaceID, "member", requestUserID(r), map[string]any{
		"team_id": uuidToString(team.ID),
	})
	writeJSON(w, http.StatusOK, teamMemberToResponse(sm))
}

// ── Team Leader Evaluation ──────────────────────────────────────────────────

// RecordTeamLeaderEvaluation records a team leader's evaluation decision
// into the unified activity_log. Called by the leader agent via CLI after
// each trigger to record whether it took action, stayed silent, or failed.
//
// The leader-turn check is task-provenance based (is_leader_task + team_id on
// the X-Task-ID row), the SAME source the claim path uses to inject the team
// briefing and the mandatory-recording instruction. The target issue's own
// assignee is deliberately NOT consulted: leaders legitimately run on issues
// that are not team-assigned. See MUL-6622 / GH #7487.
//
// Two authorization gates, in this order: the caller must own the task, and must
// still be the team's leader. The first is what makes it safe to quote the
// task's issue id in an error; the second is what keeps a claim-downgraded run
// (see the comment at gate 2) from writing a leader verdict.
func (h *Handler) RecordTeamLeaderEvaluation(w http.ResponseWriter, r *http.Request) {
	issue, ok := h.loadIssueForUser(w, r, chi.URLParam(r, "id"))
	if !ok {
		return
	}

	var req struct {
		Outcome string `json:"outcome"` // action | no_action | failed
		Reason  string `json:"reason"`  // short explanation from leader
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	if req.Outcome != "action" && req.Outcome != "no_action" && req.Outcome != "failed" {
		writeError(w, http.StatusBadRequest, "outcome must be 'action', 'no_action', or 'failed'")
		return
	}

	// Authority for "is this run a team leader turn" is the TASK ROW, not the
	// target issue's assignee (MUL-6622 / GH #7487). is_leader_task + team_id
	// are stamped at enqueue time and are exactly what the claim path keys the
	// team briefing and the mandatory-`team activity` instruction off
	// (handler/daemon.go, daemon/prompt.go taskIsTeamLeader). Gating this
	// endpoint on issue.executor_type == "team" instead made the recording
	// call unsatisfiable on the paths where a leader legitimately runs on a
	// non-team-assigned issue — a `@team` mention on an issue owned by a
	// plain agent, or a leader task bound to a child issue — and because the
	// no_action instruction forbids substituting a comment, the decision left
	// no trace at all.
	//
	// Check ORDER is load-bearing. Every rejection below the ownership gate may
	// quote task-derived ids; every rejection above it must not. `GetAgentTask`
	// is a global lookup by id, so tenant scoping (GetAgentTaskInWorkspace,
	// which joins the owning agent's workspace) and the "caller owns this task"
	// gate both come first — otherwise an unrelated task id, probed through an
	// issue the caller can legitimately read, would echo back a foreign
	// workspace's issue id.
	taskID := r.Header.Get("X-Task-ID")
	taskUUID, ok := parseUUIDOrBadRequest(w, taskID, "task id")
	if !ok {
		return
	}
	task, err := h.Queries.GetAgentTaskInWorkspace(r.Context(), db.GetAgentTaskInWorkspaceParams{
		ID:          taskUUID,
		WorkspaceID: issue.WorkspaceID,
	})
	if err != nil || !task.IssueID.Valid {
		writeError(w, http.StatusBadRequest, "task does not belong to issue")
		return
	}

	// Security gate 1: the caller must be the agent this task was enqueued for.
	workspaceID := uuidToString(issue.WorkspaceID)
	userID := requestUserID(r)
	actorType, actorID := h.resolveActor(r, userID, workspaceID)
	if actorType != "agent" || !task.AgentID.Valid || actorID != uuidToString(task.AgentID) {
		writeError(w, http.StatusForbidden, "only the team leader agent can record evaluations")
		return
	}

	// Past the ownership gate, naming the task's own issue is safe — and useful:
	// a leader woken by a stage barrier / child-done callback runs on the PARENT
	// issue, so recording against the child it just read is the common mistake.
	if uuidToString(task.IssueID) != uuidToString(issue.ID) {
		writeError(w, http.StatusBadRequest,
			"task does not belong to issue; record the evaluation on issue "+uuidToString(task.IssueID)+" (the issue this task is running on)")
		return
	}
	// Narrowing vs. the old behavior: a leader agent running a NON-leader task
	// on a team-assigned issue (for example a same-team worker task) used to
	// be accepted here. It is rejected now — it is not running as the leader,
	// and the runtime only mandates this call when taskIsTeamLeader(task).
	if !task.IsLeaderTask {
		writeError(w, http.StatusBadRequest, "task is not a team leader task")
		return
	}
	if !task.TeamID.Valid {
		// Pre-MUL-3730 rows can be leader tasks without a stamped team. Log it
		// rather than failing silently from the operator's point of view.
		slog.Warn("team leader evaluation: leader task has no team_id",
			append(logger.RequestAttrs(r),
				"task_id", uuidToString(task.ID),
				"issue_id", uuidToString(issue.ID),
			)...)
		writeError(w, http.StatusBadRequest, "leader task has no team_id")
		return
	}

	team, err := h.Queries.GetTeamInWorkspace(r.Context(), db.GetTeamInWorkspaceParams{
		ID:          task.TeamID,
		WorkspaceID: issue.WorkspaceID,
	})
	if err != nil {
		writeError(w, http.StatusNotFound, "team not found")
		return
	}

	// Security gate 2: the caller must still be this team's leader.
	//
	// `is_leader_task` on the row records the enqueue-time INTENT, which is not
	// always the role the claim actually delivered: when the leader was swapped
	// between enqueue and claim, the claim path clears resp.IsLeaderTask and the
	// run proceeds as an ordinary agent turn, while the row keeps
	// is_leader_task = true (handler/daemon.go, "claim delivered as a non-leader
	// task"). Trusting the row alone would let such a downgraded run write a
	// leader verdict — and, on no_action, suppress its own comment. Until the
	// delivered role is persisted, the live leader check is the only thing that
	// distinguishes the two, so it stays.
	//
	// Cost of the conservative choice: a leader rotated away MID-run is refused
	// here. That no longer means silence — the injected rules now tell a leader
	// whose recording call failed to leave a short comment instead.
	if actorID != uuidToString(team.LeaderID) {
		writeError(w, http.StatusForbidden, "only the team leader agent can record evaluations")
		return
	}

	details, _ := json.Marshal(map[string]string{
		"team_id": uuidToString(team.ID),
		"task_id":  util.UUIDToString(taskUUID),
		"outcome":  req.Outcome,
		"reason":   req.Reason,
	})

	activity, err := h.Queries.CreateActivity(r.Context(), db.CreateActivityParams{
		ID:          dbid.NewV7(),
		WorkspaceID: issue.WorkspaceID,
		IssueID:     issue.ID,
		ActorType:   pgtype.Text{String: "agent", Valid: true},
		// task.AgentID, not team.LeaderID: the no_action comment suppression
		// lookup matches actor_id against task.agent_id
		// (service.HasTeamLeaderNoActionEvaluationForTask), so this column has
		// to carry the task's agent for suppression to find the row at all.
		ActorID: task.AgentID,
		Action:  "team_leader_evaluated",
		Details: details,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to record evaluation")
		return
	}

	h.publish(protocol.EventActivityCreated, uuidToString(issue.WorkspaceID), "agent", actorID, map[string]any{
		"issue_id": uuidToString(issue.ID),
		"entry": map[string]any{
			"type":       "activity",
			"id":         uuidToString(activity.ID),
			"actor_type": "agent",
			"actor_id":   actorID,
			"action":     activity.Action,
			"details":    json.RawMessage(details),
			"created_at": timestampToString(activity.CreatedAt),
		},
	})

	writeJSON(w, http.StatusCreated, map[string]string{
		"id":         uuidToString(activity.ID),
		"action":     activity.Action,
		"created_at": timestampToString(activity.CreatedAt),
	})
}

// ── Team Trigger Logic ─────────────────────────────────────────────────────

// shouldSuppressTeamLeaderSelfTrigger reports whether a team leader's own
// comment should be blocked from re-enqueuing that same leader. The only
// leader-authored non-leader task allowed to wake the assigned leader is a
// same-team worker task; generic agent tasks such as direct mentions and
// thread-parent replies are not worker-role proof and must not self-trigger.
func (h *Handler) shouldSuppressTeamLeaderSelfTrigger(ctx context.Context, issueID, leaderID, teamID pgtype.UUID) bool {
	latest, err := h.Queries.GetLatestTaskRoleForIssueAndAgent(ctx, db.GetLatestTaskRoleForIssueAndAgentParams{
		IssueID: issueID,
		AgentID: leaderID,
	})
	if err != nil {
		return false
	}
	if latest.IsLeaderTask {
		return true
	}
	return !latest.TeamID.Valid || uuidToString(latest.TeamID) != uuidToString(teamID)
}

// commentMentionsAnyone returns true when the comment body contains at least
// one routing-style mention — [@Name](mention://agent|member|team|all/<id>).
// Issue cross-references (mention://issue/...) are ignored because they are
// not directed at a participant. Only the current comment is inspected —
// parent (thread root) mentions are NOT inherited here.
func commentMentionsAnyone(content string) bool {
	for _, m := range util.ParseMentions(content) {
		switch m.Type {
		case "agent", "member", "team", "all":
			return true
		}
	}
	return false
}

// The team-leader assign/promotion readiness decision now lives in the single
// service.IssueService.WillEnqueueRun predicate (MUL-3375), shared by the issue
// write paths and the preview endpoint. The former handler-local mirrors
// (shouldEnqueueTeamLeaderOnAssign / isTeamLeaderReady) were removed to stop
// the four-entry-point drift. The team enqueue side effect still flows through
// enqueueTeamLeaderTask below, which keeps the leader access gate and pending
// dedup in one place.

// enqueueTeamLeaderTask triggers the team leader agent for an issue assigned
// to a team. Assign and backlog-promotion paths use this directly; comment
// paths go through computeCommentAgentTriggers so preview and create share the
// same trigger set.
// enqueueTeamLeaderTask returns true when it actually enqueued a leader task
// (so the caller can record a handoff trace only on a real run start).
func (h *Handler) enqueueTeamLeaderTask(ctx context.Context, issue db.Issue, triggerCommentID pgtype.UUID, authorType, authorID, handoffNote string) bool {
	team, err := h.Queries.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{
		ID:          issue.ExecutorID,
		WorkspaceID: issue.WorkspaceID,
	})
	if err != nil {
		return false
	}

	// The gate must judge the SAME top-of-chain human the enqueue path will
	// persist on the leader task row, or it drifts: an agent-created issue that
	// correctly inherits its originator (MUL-4305) would still be denied here
	// if the gate used an empty originator. Member authors are their own
	// originator; for agent/system-triggered assigns we resolve the originator
	// exactly like EnqueueTaskForTeamLeader* does (via the issue's origin
	// link). triggerCommentID is always empty on the assign/promote path, so we
	// pass an invalid UUID to match. A still-unresolved originator leaves
	// leaderOriginator empty, which correctly fails closed for member/team
	// targets while a workspace target still admits the agent principal.
	leaderOriginator := ""
	if authorType == "member" {
		leaderOriginator = authorID
	} else {
		leaderOriginator = uuidToString(h.TaskService.OriginatorForIssueTask(ctx, issue, pgtype.UUID{}))
	}
	if !h.canEnqueueTeamLeader(ctx, team.LeaderID, authorType, authorID, leaderOriginator, uuidToString(issue.WorkspaceID)) {
		return false
	}

	hasPending, err := h.Queries.HasPendingTaskForIssueAndAgent(ctx, db.HasPendingTaskForIssueAndAgentParams{
		IssueID: issue.ID,
		AgentID: team.LeaderID,
		// Key dedup on the reviewed head (TEN-356).
		HeadSha: h.TaskService.ResolveIssueReviewSHAParam(ctx, issue.ID),
	})
	if err != nil || hasPending {
		return false
	}

	// triggerCommentID is always empty on the assign/promote path; the handoff
	// note rides its own task column, never trigger_comment_id.
	_ = triggerCommentID
	// The member who performed the assign/promote is the accountable human for the
	// leader run (MUL-4302 §4) — the same principal the gate above judged. An agent
	// author is not a human, so only a member actor is threaded.
	if _, err := h.TaskService.EnqueueTaskForTeamLeaderWithHandoff(ctx, issue, team.LeaderID, team.ID, handoffNote, memberActorUserID(authorType, authorID)); err != nil {
		slog.Warn("enqueue team leader task failed",
			"issue_id", uuidToString(issue.ID),
			"team_id", uuidToString(team.ID),
			"leader_id", uuidToString(team.LeaderID),
			"error", err)
		return false
	}
	return true
}
