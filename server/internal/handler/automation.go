package handler

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/analytics"
	obsmetrics "github.com/patchbay-ai/patchbay/server/internal/metrics"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

// computeNextRun delegates to the shared cron helper in the service package.
func computeNextRun(cronExpr, timezone string) (time.Time, error) {
	return service.ComputeNextRun(cronExpr, timezone)
}

// ── Response types ──────────────────────────────────────────────────────────

type AutomationResponse struct {
	ID          string  `json:"id"`
	WorkspaceID string  `json:"workspace_id"`
	Title       string  `json:"title"`
	Description *string `json:"description"`
	ProjectID   *string `json:"project_id"`
	// ExecutorType is "agent" or "team". Path A from MUL-2429: when set
	// to "team", ExecutorID points at team(id) rather than agent(id) and
	// dispatch resolves to team.leader_id at run time.
	ExecutorType       string  `json:"executor_type"`
	ExecutorID         string  `json:"executor_id"`
	Status             string  `json:"status"`
	PauseReason        *string `json:"pause_reason"`
	ExecutionMode      string  `json:"execution_mode"`
	IssueTitleTemplate *string `json:"issue_title_template"`
	CreatedByType      string  `json:"created_by_type"`
	CreatedByID        string  `json:"created_by_id"`
	LastRunAt          *string `json:"last_run_at"`
	CreatedAt          string  `json:"created_at"`
	UpdatedAt          string  `json:"updated_at"`

	// List-endpoint-only derived fields (absent on the detail/create/update
	// responses and on older servers — clients must treat them as optional).
	// Enabled triggers only; last_run_status is the most recent run's status.
	TriggerKinds  []string `json:"trigger_kinds,omitempty"`
	NextRunAt     *string  `json:"next_run_at,omitempty"`
	LastRunStatus *string  `json:"last_run_status,omitempty"`

	// Always non-nil (empty slice when no subscribers configured) so
	// frontend optional-chain rules can treat the field as authoritative.
	Subscribers []AutomationSubscriberEntry `json:"subscribers"`

	// CanWrite reports whether the requesting caller may perform write/execute
	// operations on this automation — editing, deleting, triggering, and
	// managing triggers/webhook secrets (creator, workspace owner/admin, or an
	// explicit collaborator). Nil on responses built without a caller in
	// context (older servers omit it; clients must treat absence as "unknown"
	// and fall back to attempting the action). See MUL-3807.
	CanWrite *bool `json:"can_write,omitempty"`

	// CanManageAccess reports whether the caller may manage the collaborator
	// (access) list — a narrower right held only by the creator and workspace
	// owners/admins, NOT by granted collaborators (who can write but cannot
	// re-grant). Nil when built without a caller in context. See MUL-3807.
	CanManageAccess *bool `json:"can_manage_access,omitempty"`
}

type AutomationQuotaUsageResponse struct {
	Action        string           `json:"action"`
	Used          *int64           `json:"used"`
	Reserved      *int64           `json:"reserved"`
	Total         *int64           `json:"total"`
	Limit         *int64           `json:"limit"`
	Reached       *bool            `json:"reached"`
	PeriodStart   *string          `json:"period_start"`
	PeriodEnd     *string          `json:"period_end"`
	ResetAt       *string          `json:"reset_at"`
	BlockedCounts map[string]int64 `json:"blocked_counts"`
}

// AutomationCollaboratorEntry is a member explicitly granted write access to an
// automation, surfaced on the detail response and the collaborator endpoints.
type AutomationCollaboratorEntry struct {
	UserType  string `json:"user_type"`
	UserID    string `json:"user_id"`
	GrantedBy string `json:"granted_by"`
	CreatedAt string `json:"created_at"`
}

func collaboratorToEntry(c db.AutomationCollaborator) AutomationCollaboratorEntry {
	return AutomationCollaboratorEntry{
		UserType:  c.UserType,
		UserID:    uuidToString(c.UserID),
		GrantedBy: uuidToString(c.GrantedBy),
		CreatedAt: timestampToString(c.CreatedAt),
	}
}

// user_type is restricted to "member" at the DB layer; the field is kept on
// the wire so a future expansion to agents/teams is additive, not breaking.
type AutomationSubscriberEntry struct {
	UserType  string `json:"user_type"`
	UserID    string `json:"user_id"`
	CreatedAt string `json:"created_at"`
}

type AutomationTriggerResponse struct {
	ID             string  `json:"id"`
	AutomationID    string  `json:"automation_id"`
	Kind           string  `json:"kind"`
	Enabled        bool    `json:"enabled"`
	CronExpression *string `json:"cron_expression"`
	Timezone       *string `json:"timezone"`
	NextRunAt      *string `json:"next_run_at"`
	WebhookToken   *string `json:"webhook_token"`
	// WebhookPath is computed from webhook_token. Always present for webhook
	// triggers; nil for schedule/api. Not stored — see triggerToResponse.
	WebhookPath *string `json:"webhook_path"`
	// WebhookURL is the absolute URL composed from the server's
	// PATCHBAY_PUBLIC_URL setting. Nil when the server has no public URL
	// configured; clients then build the URL themselves from webhook_path
	// plus their API base / current origin.
	WebhookURL *string `json:"webhook_url"`
	// Provider names the per-endpoint signing/dedupe convention. For now:
	// "generic" (bearer URL only, Idempotency-Key for dedupe) or "github"
	// (X-Hub-Signature-256 + X-GitHub-Delivery). Omitted for non-webhook
	// triggers.
	Provider *string `json:"provider"`
	// HasSigningSecret indicates whether a signing secret is configured on
	// the trigger. The secret itself is never returned — it is set via a
	// dedicated write-only endpoint. Always false for non-webhook triggers.
	HasSigningSecret bool `json:"has_signing_secret"`
	// SigningSecretHint is the last 4 characters of the configured secret,
	// surfaced to help operators tell two secrets apart in the UI. Nil when
	// no secret is configured.
	SigningSecretHint *string `json:"signing_secret_hint"`
	Label             *string `json:"label"`
	LastFiredAt       *string `json:"last_fired_at"`
	CreatedAt         string  `json:"created_at"`
	UpdatedAt         string  `json:"updated_at"`
	// EventFilters is the declared event scope. Only present for webhook
	// triggers; omitted when the trigger accepts all events. Serializes as
	// a JSON array of {event, actions?} objects — never as a base64 string
	// (which is what []byte would produce through encoding/json).
	EventFilters []WebhookEventFilter `json:"event_filters,omitempty"`
}

type AutomationRunResponse struct {
	ID            string  `json:"id"`
	AutomationID   string  `json:"automation_id"`
	TriggerID     *string `json:"trigger_id"`
	Source        string  `json:"source"`
	Status        string  `json:"status"`
	IssueID       *string `json:"issue_id"`
	TaskID        *string `json:"task_id"`
	TriggeredAt   string  `json:"triggered_at"`
	CompletedAt   *string `json:"completed_at"`
	FailureReason *string `json:"failure_reason"`
	// ReasonCode is a stable, localizable, enumeration-safe classification of a
	// non-success run (skipped/failed), persisted at the decision source. The UI
	// localizes it instead of echoing the raw English reason (which may name a
	// private assignee agent). Additive: nil for legacy/success-path runs.
	ReasonCode     *string `json:"reason_code,omitempty"`
	TriggerPayload any     `json:"trigger_payload"`
	Result         any     `json:"result"`
	CreatedAt      string  `json:"created_at"`
}

// ── Converters ──────────────────────────────────────────────────────────────

func automationToResponse(a db.Automation, subscribers []db.AutomationSubscriber) AutomationResponse {
	assigneeType := a.ExecutorType
	if assigneeType == "" {
		// Older rows pre-MUL-2429 may surface as "" against an out-of-date
		// schema view; default to "agent" so the API contract stays
		// non-null.
		assigneeType = "agent"
	}
	subResp := make([]AutomationSubscriberEntry, len(subscribers))
	for i, s := range subscribers {
		subResp[i] = AutomationSubscriberEntry{
			UserType:  s.UserType,
			UserID:    uuidToString(s.UserID),
			CreatedAt: timestampToString(s.CreatedAt),
		}
	}
	return AutomationResponse{
		ID:                 uuidToString(a.ID),
		WorkspaceID:        uuidToString(a.WorkspaceID),
		Title:              a.Title,
		Description:        textToPtr(a.Description),
		ProjectID:          uuidToPtr(a.ProjectID),
		ExecutorType:       assigneeType,
		ExecutorID:         uuidToString(a.ExecutorID),
		Status:             a.Status,
		PauseReason:        textToPtr(a.PauseReason),
		ExecutionMode:      a.ExecutionMode,
		IssueTitleTemplate: textToPtr(a.IssueTitleTemplate),
		CreatedByType:      a.CreatedByType,
		CreatedByID:        uuidToString(a.CreatedByID),
		LastRunAt:          timestampToPtr(a.LastRunAt),
		CreatedAt:          timestampToString(a.CreatedAt),
		UpdatedAt:          timestampToString(a.UpdatedAt),
		Subscribers:        subResp,
	}
}

func (h *Handler) triggerToResponse(t db.AutomationTrigger) AutomationTriggerResponse {
	resp := AutomationTriggerResponse{
		ID:             uuidToString(t.ID),
		AutomationID:    uuidToString(t.AutomationID),
		Kind:           t.Kind,
		Enabled:        t.Enabled,
		CronExpression: textToPtr(t.CronExpression),
		Timezone:       textToPtr(t.Timezone),
		NextRunAt:      timestampToPtr(t.NextRunAt),
		WebhookToken:   textToPtr(t.WebhookToken),
		Label:          textToPtr(t.Label),
		LastFiredAt:    timestampToPtr(t.LastFiredAt),
		CreatedAt:      timestampToString(t.CreatedAt),
		UpdatedAt:      timestampToString(t.UpdatedAt),
	}
	if t.Kind == "webhook" && t.WebhookToken.Valid && t.WebhookToken.String != "" {
		path := webhookPathForToken(t.WebhookToken.String)
		resp.WebhookPath = &path
		if h.cfg.PublicURL != "" {
			full := h.cfg.PublicURL + path
			resp.WebhookURL = &full
		}
		provider := t.Provider
		if provider == "" {
			provider = "generic"
		}
		resp.Provider = &provider
		if t.SigningSecret.Valid && t.SigningSecret.String != "" {
			resp.HasSigningSecret = true
			hint := signingSecretHint(t.SigningSecret.String)
			resp.SigningSecretHint = &hint
		}
		if len(t.EventFilters) > 0 {
			var filters []WebhookEventFilter
			if err := json.Unmarshal(t.EventFilters, &filters); err == nil {
				resp.EventFilters = filters
			}
			// On unmarshal error we deliberately drop the field instead of
			// surfacing raw bytes or 500ing — strict write-time validation
			// is supposed to make this branch unreachable, and the matcher
			// fails closed if a corrupt row ever slips through.
		}
	}
	return resp
}

// signingSecretHint returns the last 4 characters of the signing secret so a
// configured-vs-rotated state is visible in the UI without exposing the
// secret itself. Truncating below 4 chars (which the validator already
// rejects) just returns an empty string.
func signingSecretHint(secret string) string {
	if len(secret) < 4 {
		return ""
	}
	return secret[len(secret)-4:]
}

// webhookPathForToken composes the path used by the public ingress route.
// Kept as a free function (no Handler receiver) so test code that builds
// expected URLs without instantiating a Handler can call it.
func webhookPathForToken(token string) string {
	return "/api/webhooks/automations/" + token
}

func runToResponse(r db.AutomationRun) AutomationRunResponse {
	var payload any
	if r.TriggerPayload != nil {
		json.Unmarshal(r.TriggerPayload, &payload)
	}
	var result any
	if r.Result != nil {
		json.Unmarshal(r.Result, &result)
	}
	return AutomationRunResponse{
		ID:             uuidToString(r.ID),
		AutomationID:    uuidToString(r.AutomationID),
		TriggerID:      uuidToPtr(r.TriggerID),
		Source:         r.Source,
		Status:         r.Status,
		IssueID:        uuidToPtr(r.IssueID),
		TaskID:         uuidToPtr(r.TaskID),
		TriggeredAt:    timestampToString(r.TriggeredAt),
		CompletedAt:    timestampToPtr(r.CompletedAt),
		FailureReason:  textToPtr(r.FailureReason),
		ReasonCode:     textToPtr(r.ReasonCode),
		TriggerPayload: payload,
		Result:         result,
		CreatedAt:      timestampToString(r.CreatedAt),
	}
}

// runToResponseSlim mirrors runToResponse but omits TriggerPayload, intended
// for list endpoints where echoing the full webhook envelope (up to
// 256 KiB × N rows) would dominate response size. Clients fetch the full
// payload via GET /api/automations/{id}/runs/{runId} when the user opens
// the run detail dialog.
func runToResponseSlim(r db.AutomationRun) AutomationRunResponse {
	resp := runToResponse(r)
	resp.TriggerPayload = nil
	return resp
}

// ── Request types ───────────────────────────────────────────────────────────

type CreateAutomationRequest struct {
	Title       string  `json:"title"`
	Description *string `json:"description"`
	ProjectID   *string `json:"project_id"`
	// ExecutorType is optional and defaults to "agent" — preserves backward
	// compatibility with desktop clients shipped before MUL-2429.
	ExecutorType       *string           `json:"executor_type"`
	ExecutorID         string            `json:"executor_id"`
	ExecutionMode      string            `json:"execution_mode"`
	IssueTitleTemplate *string           `json:"issue_title_template"`
	Subscribers        []SubscriberInput `json:"subscribers"`
}

type UpdateAutomationRequest struct {
	Title              *string `json:"title"`
	Description        *string `json:"description"`
	ProjectID          *string `json:"project_id"`
	ExecutorType       *string `json:"executor_type"`
	ExecutorID         *string `json:"executor_id"`
	Status             *string `json:"status"`
	ExecutionMode      *string `json:"execution_mode"`
	IssueTitleTemplate *string `json:"issue_title_template"`
	// Wholesale replacement when present; omit to leave subscribers untouched.
	Subscribers []SubscriberInput `json:"subscribers"`
}

type SubscriberInput struct {
	UserType string `json:"user_type"`
	UserID   string `json:"user_id"`
}

type CreateAutomationTriggerRequest struct {
	Kind           string  `json:"kind"`
	CronExpression *string `json:"cron_expression"`
	Timezone       *string `json:"timezone"`
	Label          *string `json:"label"`
	// Provider is currently only meaningful for kind=webhook. Allowed
	// values: "generic" (default) or "github". Unset → "generic".
	Provider *string `json:"provider"`
	// EventFilters is an optional list of {event, actions?} scopes. Only
	// meaningful for webhook triggers. nil/empty means "accept all events".
	EventFilters []WebhookEventFilter `json:"event_filters,omitempty"`
}

// SetSigningSecretRequest is the body shape for PUT
// /api/automations/{id}/triggers/{triggerId}/signing-secret. Lives in its own
// type so the secret never appears alongside other fields on the trigger
// update path — handlers that log request bodies for debugging cannot pick it
// up by accident.
type SetSigningSecretRequest struct {
	// SigningSecret is the new HMAC key. Sending an empty string explicitly
	// clears the secret (disables signature verification). Pass any
	// reasonably entropic value — GitHub's docs recommend at least 32 random
	// characters; we enforce a 16-char minimum on non-empty input.
	SigningSecret string `json:"signing_secret"`
}

type UpdateAutomationTriggerRequest struct {
	Enabled        *bool   `json:"enabled"`
	CronExpression *string `json:"cron_expression"`
	Timezone       *string `json:"timezone"`
	Label          *string `json:"label"`
	// EventFilters is the desired event-filter set with tri-state PATCH
	// semantics:
	//
	//   - omitted / explicit null (nil pointer) → leave the existing value
	//     untouched.
	//   - explicit [] (non-nil, length 0)       → clear filters (the trigger
	//     reverts to "accept all events").
	//   - explicit [...]                        → replace with the supplied
	//     list.
	//
	// This is why the pointer matters: with a plain []WebhookEventFilter
	// there is no way to tell "field absent from the PATCH body" from "field
	// present but empty", and the user can never clear filters once set.
	EventFilters *[]WebhookEventFilter `json:"event_filters,omitempty"`
}

// ── Handlers ────────────────────────────────────────────────────────────────

func (h *Handler) ListAutomations(w http.ResponseWriter, r *http.Request) {
	workspaceID := h.resolveWorkspaceID(r)

	var statusFilter pgtype.Text
	if s := r.URL.Query().Get("status"); s != "" {
		statusFilter = pgtype.Text{String: s, Valid: true}
	}

	automations, err := h.Queries.ListAutomations(r.Context(), db.ListAutomationsParams{
		WorkspaceID: parseUUID(workspaceID),
		Status:      statusFilter,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list automations")
		return
	}

	// Resolve the caller's write access for per-row can_write. The collaborator
	// grants are fetched once as a set (keyed by automation id) so the flag
	// costs no per-row query. A missing member (shouldn't happen behind the
	// workspace-member middleware) just yields can_write=false everywhere.
	caller, callerErr := h.getWorkspaceMember(r.Context(), requestUserID(r), workspaceID)
	collabSet := map[string]struct{}{}
	if callerErr == nil {
		if ids, err := h.Queries.ListAutomationIDsForCollaborator(r.Context(), caller.UserID); err == nil {
			for _, id := range ids {
				collabSet[uuidToString(id)] = struct{}{}
			}
		}
	}

	// Subscribers are fetched for the whole page in one batched query. An
	// earlier version passed nil here to dodge an N+1, but the response type
	// serializes a nil slice as [] rather than omitting it, so every listed
	// automation claimed to have no subscribers while the detail endpoint
	// reported the real ones — a silently wrong value is worse than a missing
	// one (MUL-6680). The batch keys off the primary key's leading column, so
	// this costs one indexed query per page, not one per row.
	subsByAutomation := map[string][]db.AutomationSubscriber{}
	automationIDs := make([]pgtype.UUID, 0, len(automations))
	for _, row := range automations {
		automationIDs = append(automationIDs, row.Automation.ID)
	}
	if len(automationIDs) > 0 {
		subs, err := h.Queries.ListAutomationSubscribersForAutomations(r.Context(), automationIDs)
		if err != nil {
			// Fail closed. Degrading to an empty set here would reintroduce
			// exactly the bug this endpoint was fixed for: subscribers is a
			// non-omitempty field documented as authoritative, so an empty
			// value on a failed read is indistinguishable from "none
			// configured" — and a caller acting on it can overwrite a real
			// subscriber list. An error the caller can see and retry is the
			// only honest answer.
			writeError(w, http.StatusInternalServerError, "failed to list automation subscribers")
			return
		}
		for _, s := range subs {
			id := uuidToString(s.AutomationID)
			subsByAutomation[id] = append(subsByAutomation[id], s)
		}
	}

	resp := make([]AutomationResponse, len(automations))
	for i, row := range automations {
		r := automationToResponse(row.Automation, subsByAutomation[uuidToString(row.Automation.ID)])
		r.TriggerKinds = row.TriggerKinds
		if row.NextRunAt.Valid {
			r.NextRunAt = timestampToPtr(row.NextRunAt)
		}
		if row.LastRunStatus != "" {
			s := row.LastRunStatus
			r.LastRunStatus = &s
		}
		if callerErr == nil {
			_, isCollaborator := collabSet[uuidToString(row.Automation.ID)]
			cw := automationWriteByOwnership(row.Automation, caller) || isCollaborator
			r.CanWrite = &cw
		}
		resp[i] = r
	}
	writeJSON(w, http.StatusOK, map[string]any{"automations": resp, "total": len(resp)})
}

func (h *Handler) GetAutomation(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	workspaceID := h.resolveWorkspaceID(r)

	automation, ok := h.loadAutomationInWorkspace(w, r, id, workspaceID)
	if !ok {
		return
	}

	subs, err := h.Queries.ListAutomationSubscribers(r.Context(), automation.ID)
	if err != nil {
		// Fail closed for the same reason the list endpoint does: an empty
		// subscribers array is a claim, not an absence, and this response is
		// what clients round-trip back into a full-replace PATCH.
		writeError(w, http.StatusInternalServerError, "failed to list automation subscribers")
		return
	}
	resp := automationToResponse(automation, subs)

	// Resolve the caller's write access once: it both stamps can_write and
	// gates webhook-secret exposure. Webhook tokens are trigger-granting
	// secrets (anyone who reads the token can fire the automation from outside
	// the permission system), so only writers — the creator, a workspace
	// owner/admin, or a granted collaborator — get the live token/URL; every
	// other member sees the trigger metadata with the secret fields stripped
	// (MUL-3807).
	canWrite := false
	canManageAccess := false
	if member, err := h.getWorkspaceMember(r.Context(), requestUserID(r), workspaceID); err == nil {
		canWrite = h.memberCanWriteAutomation(r.Context(), automation, member)
		// Managing the access list is narrower than write: collaborators can
		// write but cannot re-grant (MUL-3807).
		canManageAccess = automationWriteByOwnership(automation, member)
	}
	resp.CanWrite = &canWrite
	resp.CanManageAccess = &canManageAccess

	// Include triggers.
	triggers, err := h.Queries.ListAutomationTriggers(r.Context(), automation.ID)
	if err != nil {
		triggers = nil
	}
	triggerResp := make([]AutomationTriggerResponse, len(triggers))
	for i, t := range triggers {
		tr := h.triggerToResponse(t)
		if !canWrite {
			tr.WebhookToken = nil
			tr.WebhookPath = nil
			tr.WebhookURL = nil
		}
		triggerResp[i] = tr
	}

	// Include the explicit collaborator grants so the "manage access" UI can
	// render the current list without a second round-trip.
	collaborators, err := h.Queries.ListAutomationCollaborators(r.Context(), automation.ID)
	if err != nil {
		collaborators = nil
	}
	collabResp := make([]AutomationCollaboratorEntry, len(collaborators))
	for i, c := range collaborators {
		collabResp[i] = collaboratorToEntry(c)
	}

	writeJSON(w, http.StatusOK, map[string]any{
		"automation":     resp,
		"triggers":      triggerResp,
		"collaborators": collabResp,
	})
}

func (h *Handler) loadAutomationInWorkspace(w http.ResponseWriter, r *http.Request, automationID, workspaceID string) (db.Automation, bool) {
	automationUUID, ok := parseUUIDOrBadRequest(w, automationID, "automation id")
	if !ok {
		return db.Automation{}, false
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace id")
	if !ok {
		return db.Automation{}, false
	}

	automation, err := h.Queries.GetAutomationInWorkspace(r.Context(), db.GetAutomationInWorkspaceParams{
		ID:          automationUUID,
		WorkspaceID: wsUUID,
	})
	if err != nil {
		writeError(w, http.StatusNotFound, "automation not found")
		return db.Automation{}, false
	}
	return automation, true
}

// automationWriteByOwnership is the implicit, query-free part of the write
// predicate: the automation's creator and workspace owners/admins always have
// write access. Explicit collaborator grants (memberCanWriteAutomation) layer
// on top of this (MUL-3807).
func automationWriteByOwnership(ap db.Automation, member db.Member) bool {
	if roleAllowed(member.Role, "owner", "admin") {
		return true
	}
	return ap.CreatedByType == "member" && uuidToString(ap.CreatedByID) == uuidToString(member.UserID)
}

// memberCanWriteAutomation reports whether the given member may perform write or
// execute operations on the automation — editing it, deleting it, triggering
// runs, replaying deliveries, and managing its triggers, webhook secrets, and
// access list. Write access is held by the automation's creator, by workspace
// owners/admins, and by members explicitly granted as collaborators. The same
// predicate also gates whether webhook secrets are exposed on the read path,
// since seeing a webhook token is equivalent to being able to trigger.
func (h *Handler) memberCanWriteAutomation(ctx context.Context, ap db.Automation, member db.Member) bool {
	if automationWriteByOwnership(ap, member) {
		return true
	}
	granted, err := h.Queries.IsAutomationCollaborator(ctx, db.IsAutomationCollaboratorParams{
		AutomationID: ap.ID,
		UserID:      member.UserID,
	})
	return err == nil && granted
}

// requireAutomationWrite enforces memberCanWriteAutomation for a mutating/
// executing request. On failure it writes the response (404 when the caller is
// not a member of the workspace, 403 otherwise) and returns false; the caller
// must return early. On success it returns true.
func (h *Handler) requireAutomationWrite(w http.ResponseWriter, r *http.Request, ap db.Automation, workspaceID string) bool {
	member, ok := h.workspaceMember(w, r, workspaceID)
	if !ok {
		return false
	}
	if !h.memberCanWriteAutomation(r.Context(), ap, member) {
		writeError(w, http.StatusForbidden, "only the automation creator, a workspace admin, or a granted collaborator can manage this automation")
		return false
	}
	return true
}

// requireAutomationAccessManagement enforces the narrower predicate used by the
// collaborator (access list) endpoints: only the automation's creator or a
// workspace owner/admin may grant or revoke access. A granted collaborator
// keeps its own write/execute rights (edit, trigger, manage triggers/secrets)
// but cannot manage the access list — this stops a collaborator from
// re-granting access to others or revoking peers (privilege escalation).
// See MUL-3807.
func (h *Handler) requireAutomationAccessManagement(w http.ResponseWriter, r *http.Request, ap db.Automation, workspaceID string) bool {
	member, ok := h.workspaceMember(w, r, workspaceID)
	if !ok {
		return false
	}
	if !automationWriteByOwnership(ap, member) {
		writeError(w, http.StatusForbidden, "only the automation creator or a workspace admin can manage access")
		return false
	}
	return true
}

func (h *Handler) CreateAutomation(w http.ResponseWriter, r *http.Request) {
	var req CreateAutomationRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.Title == "" {
		writeError(w, http.StatusBadRequest, "title is required")
		return
	}
	if req.ExecutorID == "" {
		writeError(w, http.StatusBadRequest, "executor_id is required")
		return
	}
	if req.ExecutionMode == "" {
		writeError(w, http.StatusBadRequest, "execution_mode is required")
		return
	}
	if req.ExecutionMode != "create_issue" && req.ExecutionMode != "run_only" {
		writeError(w, http.StatusBadRequest, "execution_mode must be create_issue or run_only")
		return
	}
	if req.IssueTitleTemplate != nil {
		if err := service.ValidateIssueTitleTemplate(*req.IssueTitleTemplate); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
	}

	workspaceID := h.resolveWorkspaceID(r)
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}

	assigneeUUID, ok := parseUUIDOrBadRequest(w, req.ExecutorID, "executor_id")
	if !ok {
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace id")
	if !ok {
		return
	}

	assigneeType := "agent"
	if req.ExecutorType != nil && *req.ExecutorType != "" {
		assigneeType = *req.ExecutorType
	}
	if !isValidAutomationAssigneeType(assigneeType) {
		writeError(w, http.StatusBadRequest, "executor_type must be agent or team")
		return
	}
	projectID, ok := h.parseAutomationProjectID(w, r, req.ProjectID, wsUUID)
	if !ok {
		return
	}

	// Parse before insert so a malformed payload doesn't open a transaction.
	subscribers, ok := parseAutomationSubscribers(w, req.Subscribers)
	if !ok {
		return
	}

	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create automation")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	// This must be the first lock family in the transaction. Member revocation
	// takes the same per-(workspace, user) locks before pruning templates and
	// deleting member rows. Re-checking membership after those locks means
	// either this save commits first and revocation prunes it, or revocation
	// commits first and this save rejects the departed subscriber.
	if !h.lockAndValidateAutomationSubscribers(w, r, qtx, subscribers, wsUUID) {
		return
	}

	// Keep save-time readiness validation in the same transaction as the
	// insert. The assignment lock serializes this path with Runtime teardown,
	// so an active Automation cannot slip in after teardown's pause sweep.
	if !h.validateAutomationAssigneeForSave(w, r, qtx, assigneeType, assigneeUUID, wsUUID, true) {
		return
	}

	automation, err := qtx.CreateAutomation(r.Context(), db.CreateAutomationParams{
		WorkspaceID:        wsUUID,
		Title:              req.Title,
		ExecutorType:       assigneeType,
		ExecutorID:         assigneeUUID,
		Status:             "active",
		ExecutionMode:      req.ExecutionMode,
		CreatedByType:      "member",
		CreatedByID:        parseUUID(userID),
		Description:        ptrToText(req.Description),
		IssueTitleTemplate: ptrToText(req.IssueTitleTemplate),
		ProjectID:          projectID,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create automation")
		return
	}

	// Creating an automation IS a substantive publish: append rule-version v1 with
	// the creating member as publisher, so every automation has an accountable
	// human at dispatch time (MUL-4302 §3.4).
	if err := h.recordAutomationRuleVersion(r.Context(), qtx, automation, "member", parseUUID(userID)); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create automation")
		return
	}

	for _, subscriber := range subscribers {
		if err := qtx.AddAutomationSubscriber(r.Context(), db.AddAutomationSubscriberParams{
			AutomationID: automation.ID,
			UserType:    "member",
			UserID:      subscriber.UserID,
		}); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to add automation subscriber")
			return
		}
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create automation")
		return
	}
	subs, err := h.Queries.ListAutomationSubscribers(r.Context(), automation.ID)
	if err != nil {
		subs = nil
	}

	resp := automationToResponse(automation, subs)
	h.publish(protocol.EventAutomationCreated, workspaceID, "member", userID, map[string]any{"automation": resp})
	obsmetrics.RecordEvent(h.Analytics, h.Metrics, analytics.AutomationCreated(
		userID,
		workspaceID,
		uuidToString(automation.ID),
		"manual",
		"manual",
	))
	writeJSON(w, http.StatusCreated, resp)
}

type automationSubscriberCandidate struct {
	UserID     pgtype.UUID
	InputIndex int
}

// parseAutomationSubscribers validates the wire shape without reading mutable
// membership state. Membership is checked only after the write transaction
// owns the same serialization locks as member revocation.
func parseAutomationSubscribers(w http.ResponseWriter, raw []SubscriberInput) ([]automationSubscriberCandidate, bool) {
	if len(raw) == 0 {
		return nil, true
	}
	out := make([]automationSubscriberCandidate, 0, len(raw))
	seen := make(map[string]bool, len(raw))
	for i, entry := range raw {
		if entry.UserType != "member" {
			writeError(w, http.StatusBadRequest, fmt.Sprintf("subscribers[%d].user_type must be 'member'", i))
			return nil, false
		}
		if entry.UserID == "" {
			writeError(w, http.StatusBadRequest, fmt.Sprintf("subscribers[%d].user_id is required", i))
			return nil, false
		}
		uid, ok := parseUUIDOrBadRequest(w, entry.UserID, fmt.Sprintf("subscribers[%d].user_id", i))
		if !ok {
			return nil, false
		}
		canonicalID := uuidToString(uid)
		if seen[canonicalID] {
			continue
		}
		seen[canonicalID] = true
		out = append(out, automationSubscriberCandidate{UserID: uid, InputIndex: i})
	}
	return out, true
}

// lockAndValidateAutomationSubscribers serializes subscriber-template writes
// with member revocation. Locks and row checks both use canonical UUID order,
// so two saves containing the same members in different request orders cannot
// deadlock each other.
func (h *Handler) lockAndValidateAutomationSubscribers(
	w http.ResponseWriter,
	r *http.Request,
	qtx *db.Queries,
	subscribers []automationSubscriberCandidate,
	workspaceID pgtype.UUID,
) bool {
	ordered := append([]automationSubscriberCandidate(nil), subscribers...)
	sort.Slice(ordered, func(i, j int) bool {
		return uuidToString(ordered[i].UserID) < uuidToString(ordered[j].UserID)
	})

	for _, subscriber := range ordered {
		if err := qtx.LockSubscriberWrites(r.Context(), db.LockSubscriberWritesParams{
			WorkspaceID: workspaceID,
			UserID:      subscriber.UserID,
		}); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to validate automation subscribers")
			return false
		}
	}
	for _, subscriber := range ordered {
		if _, err := qtx.LockActiveMember(r.Context(), db.LockActiveMemberParams{
			UserID:      subscriber.UserID,
			WorkspaceID: workspaceID,
		}); err != nil {
			if errors.Is(err, pgx.ErrNoRows) {
				writeError(w, http.StatusBadRequest, fmt.Sprintf(
					"subscribers[%d] is not a member of this workspace", subscriber.InputIndex,
				))
				return false
			}
			writeError(w, http.StatusInternalServerError, "failed to validate automation subscribers")
			return false
		}
	}
	return true
}

func (h *Handler) UpdateAutomation(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	workspaceID := h.resolveWorkspaceID(r)

	prev, ok := h.loadAutomationInWorkspace(w, r, id, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationWrite(w, r, prev, workspaceID) {
		return
	}

	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}

	bodyBytes, err := io.ReadAll(r.Body)
	if err != nil {
		writeError(w, http.StatusBadRequest, "failed to read request body")
		return
	}
	var req UpdateAutomationRequest
	if err := json.Unmarshal(bodyBytes, &req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	var rawFields map[string]json.RawMessage
	json.Unmarshal(bodyBytes, &rawFields)

	params := db.UpdateAutomationParams{
		ID:                 prev.ID,
		Description:        prev.Description,
		ExecutorID:         prev.ExecutorID,
		IssueTitleTemplate: prev.IssueTitleTemplate,
		ProjectID:          prev.ProjectID,
	}
	if req.Title != nil {
		params.Title = pgtype.Text{String: *req.Title, Valid: true}
	}
	if req.Status != nil {
		params.Status = pgtype.Text{String: *req.Status, Valid: true}
	}
	if req.ExecutionMode != nil {
		params.ExecutionMode = pgtype.Text{String: *req.ExecutionMode, Valid: true}
	}
	if _, ok := rawFields["description"]; ok {
		params.Description = ptrToText(req.Description)
	}
	if _, ok := rawFields["issue_title_template"]; ok {
		if req.IssueTitleTemplate != nil {
			if err := service.ValidateIssueTitleTemplate(*req.IssueTitleTemplate); err != nil {
				writeError(w, http.StatusBadRequest, err.Error())
				return
			}
		}
		params.IssueTitleTemplate = ptrToText(req.IssueTitleTemplate)
	}
	if _, ok := rawFields["project_id"]; ok {
		projectID, ok := h.parseAutomationProjectID(w, r, req.ProjectID, prev.WorkspaceID)
		if !ok {
			return
		}
		params.ProjectID = projectID
	}
	// executor_type and executor_id are validated as a pair: switching
	// between agent and team without supplying a new id would leave the
	// row pointing at the wrong table. The client is expected to send both
	// fields on any change; partial updates that change only one are
	// rejected.
	_, typeSent := rawFields["executor_type"]
	_, idSent := rawFields["executor_id"]
	nextType := prev.ExecutorType
	nextID := prev.ExecutorID
	if typeSent || idSent {
		if typeSent && req.ExecutorType != nil && *req.ExecutorType != "" {
			nextType = *req.ExecutorType
		}
		if !isValidAutomationAssigneeType(nextType) {
			writeError(w, http.StatusBadRequest, "executor_type must be agent or team")
			return
		}
		if idSent {
			if req.ExecutorID == nil {
				writeError(w, http.StatusBadRequest, "executor_id cannot be null")
				return
			}
			parsed, ok := parseUUIDOrBadRequest(w, *req.ExecutorID, "executor_id")
			if !ok {
				return
			}
			nextID = parsed
		}
		// Reject the agent↔team switch without a paired id, otherwise the
		// row would address agent(id) under executor_type='team' or vice
		// versa.
		if typeSent && !idSent && nextType != prev.ExecutorType {
			writeError(w, http.StatusBadRequest, "executor_id is required when changing executor_type")
			return
		}
		if typeSent {
			params.ExecutorType = pgtype.Text{String: nextType, Valid: true}
		}
		if idSent {
			params.ExecutorID = nextID
		}
	}

	// Parse subscriber wire values up-front. Mutable membership is revalidated
	// under the revocation serialization locks after the transaction starts.
	var (
		subscribers        []automationSubscriberCandidate
		replaceSubscribers bool
	)
	if _, sent := rawFields["subscribers"]; sent {
		replaceSubscribers = true
		validated, vok := parseAutomationSubscribers(w, req.Subscribers)
		if !vok {
			return
		}
		subscribers = validated
	}

	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update automation")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	// Lock subscriber identities before assignment and automation rows. This is
	// the same global order used by CreateAutomation and member revocation.
	if !h.lockAndValidateAutomationSubscribers(w, r, qtx, subscribers, prev.WorkspaceID) {
		return
	}

	// Retargeting must validate the polymorphic reference; resuming must also
	// validate Runtime readiness. Keep both in this transaction so Runtime
	// teardown either pauses an active row after it commits or wins first and
	// makes activation fail with a useful recovery message.
	nextStatus := prev.Status
	if req.Status != nil {
		nextStatus = *req.Status
	}
	validateAssignee := typeSent || idSent || (req.Status != nil && *req.Status == "active")
	if validateAssignee && !h.validateAutomationAssigneeForSave(
		w, r, qtx, nextType, nextID, prev.WorkspaceID, nextStatus == "active",
	) {
		return
	}

	// Assignment locks come first. Runtime teardown and team leader changes
	// also lock Agent/Team before they update matching Automation rows; keeping
	// that global order prevents an Agent↔Automation deadlock.
	lockedPrev, err := qtx.LockAutomationForUpdate(r.Context(), db.LockAutomationForUpdateParams{
		ID:          prev.ID,
		WorkspaceID: prev.WorkspaceID,
	})
	if errors.Is(err, pgx.ErrNoRows) {
		writeError(w, http.StatusNotFound, "automation not found")
		return
	}
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update automation")
		return
	}
	if lockedPrev.UpdatedAt.Valid != prev.UpdatedAt.Valid ||
		(lockedPrev.UpdatedAt.Valid && !lockedPrev.UpdatedAt.Time.Equal(prev.UpdatedAt.Time)) {
		writeJSON(w, http.StatusConflict, map[string]any{
			"error": "the automation changed while it was being edited; reload and try again.",
			"code":  "automation_update_conflict",
		})
		return
	}

	automation, err := qtx.UpdateAutomation(r.Context(), params)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update automation")
		return
	}

	// A substantive change (target / enabled-state / execution mode) republishes the
	// rule: append a new version with THIS member as publisher, so a later run
	// attributes to whoever last changed what the rule does — not the original
	// creator. Cosmetic edits (title / description / template) write no version and
	// leave accountability with the previous publisher (MUL-4302 §3.4).
	if automationRuleSubstantiveChange(prev, automation) {
		if err := h.recordAutomationRuleVersion(r.Context(), qtx, automation, "member", parseUUID(userID)); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to update automation")
			return
		}
		// An automation-level substantive edit governs every trigger, so responsibility
		// for each firing trigger transfers to this editor (source=trigger_owner). A
		// trigger-scoped edit re-stamps only its own row (see UpdateAutomationTrigger).
		if err := qtx.SetAutomationTriggerPublishersByAutomation(r.Context(), db.SetAutomationTriggerPublishersByAutomationParams{
			AutomationID:     automation.ID,
			PublishedByType: pgtype.Text{String: "member", Valid: true},
			PublishedByID:   parseUUID(userID),
		}); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to update automation")
			return
		}
	}

	if replaceSubscribers {
		if err := qtx.DeleteAutomationSubscribersForAutomation(r.Context(), automation.ID); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to update subscribers")
			return
		}
		for _, subscriber := range subscribers {
			if err := qtx.AddAutomationSubscriber(r.Context(), db.AddAutomationSubscriberParams{
				AutomationID: automation.ID,
				UserType:    "member",
				UserID:      subscriber.UserID,
			}); err != nil {
				writeError(w, http.StatusInternalServerError, "failed to add automation subscriber")
				return
			}
		}
	}

	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update automation")
		return
	}

	subs, err := h.Queries.ListAutomationSubscribers(r.Context(), automation.ID)
	if err != nil {
		subs = nil
	}
	resp := automationToResponse(automation, subs)
	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", userID, map[string]any{"automation": resp})
	writeJSON(w, http.StatusOK, resp)
}

// automationRuleSubstantiveChange reports whether a substantive (publish-worthy)
// field of the automation ROW changed between prev and next — a change that alters
// WHAT the automation instructs the agent to do, or WHO / WHETHER it runs, and so
// transfers accountability to the editor (MUL-4302 §3.4; boundary pinned with Elon):
//
//   - executor_type / executor_id — who (agent / team leader) executes;
//   - status — enabled state (active / paused / archived);
//   - execution_mode — run_only vs create_issue;
//   - description — the product surfaces this as the run PROMPT, i.e. the task
//     instruction itself, so editing it must transfer responsibility (the gap Elon
//     flagged: a fresh publisher of the instructions is the accountable human);
//   - issue_title_template — templates the created issue in create_issue mode; part
//     of the instruction / output spec the run produces.
//
// Deliberately NOT substantive (cosmetic / routing — they change neither the
// instruction nor the executor): title (display label) and project_id (which project
// created issues are filed under). The comparison is faithful because UpdateAutomation
// seeds every param from prev, so an omitted field round-trips unchanged.
//
// Trigger-table edits (cron / timezone / enabled / event_filters) are substantive PER
// TRIGGER and handled in UpdateAutomationTrigger; archive and system-pause republish in
// their own paths.
func automationRuleSubstantiveChange(prev, next db.Automation) bool {
	return prev.ExecutorType != next.ExecutorType ||
		prev.ExecutorID != next.ExecutorID ||
		prev.Status != next.Status ||
		prev.ExecutionMode != next.ExecutionMode ||
		prev.Description != next.Description ||
		prev.IssueTitleTemplate != next.IssueTitleTemplate
}

// recordAutomationRuleVersion appends one rule-version snapshot for a substantive
// publish (MUL-4302 §3.4). Thin handler wrapper over service.RecordAutomationRuleVersion
// (shared with the failure monitor); callers pass their tx-scoped Queries so the
// version is atomic with the automation write.
func (h *Handler) recordAutomationRuleVersion(ctx context.Context, q *db.Queries, ap db.Automation, publishedByType string, publishedByID pgtype.UUID) error {
	return service.RecordAutomationRuleVersion(ctx, q, ap, publishedByType, publishedByID)
}

func (h *Handler) parseAutomationProjectID(
	w http.ResponseWriter,
	r *http.Request,
	raw *string,
	workspaceID pgtype.UUID,
) (pgtype.UUID, bool) {
	if raw == nil || *raw == "" {
		return pgtype.UUID{}, true
	}
	projectID, ok := parseUUIDOrBadRequest(w, *raw, "project_id")
	if !ok {
		return pgtype.UUID{}, false
	}
	if _, err := h.Queries.GetProjectInWorkspace(r.Context(), db.GetProjectInWorkspaceParams{
		ID:          projectID,
		WorkspaceID: workspaceID,
	}); err != nil {
		writeError(w, http.StatusBadRequest, "project_id must reference a project in this workspace")
		return pgtype.UUID{}, false
	}
	return projectID, true
}

func (h *Handler) DeleteAutomation(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	workspaceID := h.resolveWorkspaceID(r)

	idUUID, ok := parseUUIDOrBadRequest(w, id, "automation id")
	if !ok {
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace id")
	if !ok {
		return
	}

	ap, err := h.Queries.GetAutomationInWorkspace(r.Context(), db.GetAutomationInWorkspaceParams{
		ID:          idUUID,
		WorkspaceID: wsUUID,
	})
	if err != nil {
		writeError(w, http.StatusNotFound, "automation not found")
		return
	}
	if !h.requireAutomationWrite(w, r, ap, workspaceID) {
		return
	}

	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}

	// Product "delete" is archival: stop future triggers and hide the
	// automation from default lists while preserving runs, tasks, webhook
	// deliveries, subscribers, and collaborators as execution history.
	// Archiving is a substantive status change (MUL-4302 §3.4), so republish the
	// rule version with this member as publisher, atomically with the archive.
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete automation")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	if err := qtx.ArchiveAutomation(r.Context(), idUUID); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete automation")
		return
	}
	ap.Status = "archived" // reflect the post-archive state in the version snapshot
	if err := h.recordAutomationRuleVersion(r.Context(), qtx, ap, "member", parseUUID(userID)); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete automation")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete automation")
		return
	}

	h.publish(protocol.EventAutomationDeleted, workspaceID, "member", userID, map[string]any{"automation_id": uuidToString(idUUID)})
	w.WriteHeader(http.StatusNoContent)
}

// ── Collaborator (access grant) management ───────────────────────────────────

type AutomationCollaboratorRequest struct {
	UserID string `json:"user_id"`
}

func (h *Handler) writeAutomationCollaborators(w http.ResponseWriter, r *http.Request, automationID pgtype.UUID, status int) {
	collaborators, err := h.Queries.ListAutomationCollaborators(r.Context(), automationID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load collaborators")
		return
	}
	resp := make([]AutomationCollaboratorEntry, len(collaborators))
	for i, c := range collaborators {
		resp[i] = collaboratorToEntry(c)
	}
	writeJSON(w, status, map[string]any{"collaborators": resp})
}

// AddAutomationCollaborator grants a workspace member explicit write access to
// the automation. Only the automation's creator or a workspace owner/admin can
// manage the access list; a granted collaborator cannot re-grant to others
// (privilege escalation). See MUL-3807.
func (h *Handler) AddAutomationCollaborator(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	workspaceID := h.resolveWorkspaceID(r)

	ap, ok := h.loadAutomationInWorkspace(w, r, id, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationAccessManagement(w, r, ap, workspaceID) {
		return
	}

	var req AutomationCollaboratorRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.UserID == "" {
		writeError(w, http.StatusBadRequest, "user_id is required")
		return
	}
	targetUUID, ok := parseUUIDOrBadRequest(w, req.UserID, "user_id")
	if !ok {
		return
	}
	// Only workspace members can be granted access — agents already reach
	// automations through their own dispatch path, not this grant list.
	if !h.isWorkspaceEntity(r.Context(), "member", req.UserID, workspaceID) {
		writeError(w, http.StatusBadRequest, "user_id must be a member of this workspace")
		return
	}

	grantedBy, ok := requireUserID(w, r)
	if !ok {
		return
	}
	grantedByUUID, ok := parseUUIDOrBadRequest(w, grantedBy, "granted_by")
	if !ok {
		return
	}

	if _, err := h.Queries.AddAutomationCollaborator(r.Context(), db.AddAutomationCollaboratorParams{
		AutomationID: ap.ID,
		UserType:    "member",
		UserID:      targetUUID,
		GrantedBy:   grantedByUUID,
	}); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to grant access")
		return
	}

	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", grantedBy, map[string]any{
		"automation_id": uuidToString(ap.ID),
	})
	h.writeAutomationCollaborators(w, r, ap.ID, http.StatusCreated)
}

// RemoveAutomationCollaborator revokes a member's explicit write grant. Only the
// automation's creator or a workspace owner/admin can manage the access list; a
// collaborator cannot revoke peers. Implicit writers (creator / owner / admin)
// are unaffected — there is no row to remove.
func (h *Handler) RemoveAutomationCollaborator(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	userID := chi.URLParam(r, "userId")
	workspaceID := h.resolveWorkspaceID(r)

	ap, ok := h.loadAutomationInWorkspace(w, r, id, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationAccessManagement(w, r, ap, workspaceID) {
		return
	}
	targetUUID, ok := parseUUIDOrBadRequest(w, userID, "user id")
	if !ok {
		return
	}

	if err := h.Queries.DeleteAutomationCollaborator(r.Context(), db.DeleteAutomationCollaboratorParams{
		AutomationID: ap.ID,
		UserType:    "member",
		UserID:      targetUUID,
	}); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to revoke access")
		return
	}

	actor := requestUserID(r)
	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", actor, map[string]any{
		"automation_id": uuidToString(ap.ID),
	})
	h.writeAutomationCollaborators(w, r, ap.ID, http.StatusOK)
}

// ── Trigger management ──────────────────────────────────────────────────────

func (h *Handler) CreateAutomationTrigger(w http.ResponseWriter, r *http.Request) {
	automationID := chi.URLParam(r, "id")
	workspaceID := h.resolveWorkspaceID(r)

	ap, ok := h.loadAutomationInWorkspace(w, r, automationID, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationWrite(w, r, ap, workspaceID) {
		return
	}
	// A new trigger changes what / when the rule fires — a substantive publish, so
	// the acting member republishes the rule version ATOMICALLY with the trigger
	// create (MUL-4302 §3.4). Resolved here so both the webhook and schedule create
	// paths can write the version inside the same tx as the INSERT — a failed
	// version write must roll the trigger back, never leave future dispatches
	// attributed to the previous publisher.
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	publisherID := parseUUID(userID)

	var req CreateAutomationTriggerRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.Kind == "" {
		writeError(w, http.StatusBadRequest, "kind is required")
		return
	}
	if req.Kind != "schedule" && req.Kind != "webhook" {
		// "api" kind is deprecated: it was reserved-but-inert (no scheduler,
		// no ingress route), and the only way to actually fire one was via
		// the manual /trigger endpoint — which already works regardless of
		// trigger kind. Surface stragglers with 400 so callers move to
		// schedule or webhook.
		writeError(w, http.StatusBadRequest, "kind must be schedule or webhook")
		return
	}
	if req.Kind == "schedule" && (req.CronExpression == nil || *req.CronExpression == "") {
		writeError(w, http.StatusBadRequest, "cron_expression is required for schedule triggers")
		return
	}
	if req.Kind == "webhook" && req.Timezone != nil && *req.Timezone != "" {
		// Webhook triggers fire on demand from external POSTs — they have no
		// next_run_at to compute, so a timezone is meaningless. Reject loudly
		// instead of silently dropping the field.
		writeError(w, http.StatusBadRequest, "timezone is not valid for webhook triggers")
		return
	}
	if req.Kind != "webhook" && len(req.EventFilters) > 0 {
		// event_filters narrows webhook ingress — it has no meaning for a
		// schedule trigger and would otherwise be silently dropped.
		writeError(w, http.StatusBadRequest, "event_filters is only valid for webhook triggers")
		return
	}
	if err := validateWebhookEventFilters(req.EventFilters); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	// Provider only applies to webhook triggers and the value space is
	// closed — reject unknowns early so a typo on create doesn't quietly
	// degrade into a "generic" trigger that bypasses provider-specific
	// dedupe / signature behaviour.
	provider := "generic"
	if req.Provider != nil && *req.Provider != "" {
		if req.Kind != "webhook" {
			writeError(w, http.StatusBadRequest, "provider is only valid for webhook triggers")
			return
		}
		if !isAllowedWebhookProvider(*req.Provider) {
			writeError(w, http.StatusBadRequest, "provider must be generic or github")
			return
		}
		provider = *req.Provider
	}

	if req.Timezone != nil && *req.Timezone != "" {
		if err := service.ValidateTimezone(*req.Timezone); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
	}

	// kind-specific normalization. Webhook triggers ignore cron/timezone/
	// next_run_at — they're fired on demand.
	var (
		nextRunAt    pgtype.Timestamptz
		cronText     pgtype.Text
		tzText       pgtype.Text
		webhookToken pgtype.Text
	)
	switch req.Kind {
	case "schedule":
		cronText = ptrToText(req.CronExpression)
		tzText = ptrToText(req.Timezone)
		tz := "UTC"
		if req.Timezone != nil && *req.Timezone != "" {
			tz = *req.Timezone
		}
		t, err := computeNextRun(*req.CronExpression, tz)
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		nextRunAt = pgtype.Timestamptz{Time: t, Valid: true}
	case "webhook":
		// Mint the token BEFORE the INSERT so the row never exists in a
		// half-written kind=webhook + webhook_token=NULL state. If the
		// random token happens to collide with an existing unique-index
		// entry (vanishingly unlikely with 256 bits but the retry keeps
		// the failure mode obvious if RNG is degraded), we re-generate
		// and re-INSERT — never UPDATE.
		eventFiltersBytes, err := encodeWebhookEventFilters(req.EventFilters)
		if err != nil {
			writeError(w, http.StatusInternalServerError, "failed to encode event_filters")
			return
		}
		trigger, err := h.createWebhookTriggerWithMintedToken(r, ap, ptrToText(req.Label), provider, eventFiltersBytes, publisherID)
		if err != nil {
			writeError(w, http.StatusInternalServerError, "failed to create trigger")
			return
		}
		resp := h.triggerToResponse(trigger)
		h.publish(protocol.EventAutomationUpdated, workspaceID, "member", userID, map[string]any{
			"automation_id": uuidToString(ap.ID),
			"trigger":      resp,
		})
		writeJSON(w, http.StatusCreated, resp)
		return
	}

	// Schedule create: write the trigger and republish the rule version atomically.
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create trigger")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	trigger, err := qtx.CreateAutomationTrigger(r.Context(), db.CreateAutomationTriggerParams{
		AutomationID:    ap.ID,
		Kind:           req.Kind,
		Enabled:        true,
		CronExpression: cronText,
		Timezone:       tzText,
		NextRunAt:      nextRunAt,
		Label:          ptrToText(req.Label),
		WebhookToken:   webhookToken,
		// Seed the responsible publisher = creator; a later substantive edit re-stamps
		// it to the editor so runs attribute to whoever last shaped this trigger
		// (source=trigger_owner, MUL-4302).
		PublishedByType: pgtype.Text{String: "member", Valid: publisherID.Valid},
		PublishedByID:   publisherID,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create trigger")
		return
	}
	if err := h.recordAutomationRuleVersion(r.Context(), qtx, ap, "member", publisherID); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create trigger")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to create trigger")
		return
	}

	resp := h.triggerToResponse(trigger)
	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", userID, map[string]any{
		"automation_id": uuidToString(ap.ID),
		"trigger":      resp,
	})
	writeJSON(w, http.StatusCreated, resp)
}

// createWebhookTriggerWithMintedToken atomically creates a webhook trigger
// with a freshly minted bearer token in the same INSERT. Avoids the older
// two-step (INSERT then UPDATE webhook_token) pattern which could leave a
// kind=webhook row with NULL webhook_token visible in the UI if the second
// statement failed.
//
// Each attempt runs in its OWN transaction so the trigger INSERT and the
// rule-version republish (a webhook trigger is a substantive change to what fires,
// MUL-4302 §3.4, published by publisherID) commit together — a version-write failure
// rolls the trigger back rather than leaving future dispatches attributed to the
// previous publisher. Retries on the unique-index collision case with a fresh token
// (the collided attempt's tx is already rolled back), so a vanishingly-rare RNG
// collision turns into a clean retry rather than a 500.
func (h *Handler) createWebhookTriggerWithMintedToken(
	r *http.Request,
	ap db.Automation,
	label pgtype.Text,
	provider string,
	eventFilters []byte,
	publisherID pgtype.UUID,
) (db.AutomationTrigger, error) {
	ctx := r.Context()
	for attempt := 0; attempt < 3; attempt++ {
		token, err := generateWebhookToken()
		if err != nil {
			return db.AutomationTrigger{}, err
		}
		tx, err := h.TxStarter.Begin(ctx)
		if err != nil {
			return db.AutomationTrigger{}, err
		}
		qtx := h.Queries.WithTx(tx)
		trigger, err := qtx.CreateAutomationTrigger(ctx, db.CreateAutomationTriggerParams{
			AutomationID:  ap.ID,
			Kind:         "webhook",
			Enabled:      true,
			Label:        label,
			WebhookToken: pgtype.Text{String: token, Valid: true},
			Provider:     pgtype.Text{String: provider, Valid: provider != ""},
			EventFilters: eventFilters,
			// Seed the responsible publisher = creator; re-stamped to the editor on a
			// later substantive edit (source=trigger_owner, MUL-4302).
			PublishedByType: pgtype.Text{String: "member", Valid: publisherID.Valid},
			PublishedByID:   publisherID,
		})
		if err != nil {
			tx.Rollback(ctx)
			if isUniqueViolation(err) {
				continue // token collision: retry with a fresh token
			}
			return db.AutomationTrigger{}, err
		}
		if err := h.recordAutomationRuleVersion(ctx, qtx, ap, "member", publisherID); err != nil {
			tx.Rollback(ctx)
			return db.AutomationTrigger{}, err
		}
		if err := tx.Commit(ctx); err != nil {
			return db.AutomationTrigger{}, err
		}
		return trigger, nil
	}
	return db.AutomationTrigger{}, fmt.Errorf("could not mint unique webhook token")
}

func isAllowedWebhookProvider(p string) bool {
	switch p {
	case "generic", "github":
		return true
	default:
		return false
	}
}

func isValidAutomationAssigneeType(t string) bool {
	switch t {
	case "agent", "team":
		return true
	default:
		return false
	}
}

// validateAutomationAssigneeForSave checks that the assignee (agent or team)
// exists in the given workspace and, when requireRuntime is true, that its
// effective Agent has a Runtime. It takes assignment locks through q so active
// saves and the caller's Automation write are serialized with Runtime teardown.
//
// At dispatch time the same checks (resolveAutomationLeader + AgentReadiness)
// run again — they live there to handle "leader was online at save time but
// went offline by trigger time". Save-time validation exists so the user gets
// immediate feedback ("bind a runtime first") instead of discovering the
// Automation is inert at the next schedule tick.
func (h *Handler) validateAutomationAssigneeForSave(
	w http.ResponseWriter,
	r *http.Request,
	q *db.Queries,
	assigneeType string,
	assigneeID, workspaceID pgtype.UUID,
	requireRuntime bool,
) bool {
	switch assigneeType {
	case "agent":
		agent, err := q.LockAgentForAutomationAssignment(r.Context(), db.LockAgentForAutomationAssignmentParams{
			ID:          assigneeID,
			WorkspaceID: workspaceID,
		})
		if err != nil {
			writeError(w, http.StatusBadRequest, "assignee must be a valid agent in this workspace")
			return false
		}
		if agent.ArchivedAt.Valid {
			writeError(w, http.StatusUnprocessableEntity, "assignee agent is archived; pick a different agent")
			return false
		}
		if requireRuntime && !agent.RuntimeID.Valid {
			writeError(w, http.StatusUnprocessableEntity, "assignee agent needs a runtime before this automation can be active")
			return false
		}
		return true
	case "team":
		team, err := q.LockTeamForAutomationAssignment(r.Context(), db.LockTeamForAutomationAssignmentParams{
			ID:          assigneeID,
			WorkspaceID: workspaceID,
		})
		if err != nil {
			writeError(w, http.StatusBadRequest, "assignee must be a valid team in this workspace")
			return false
		}
		// Archived teams must be rejected at save time: the dispatcher will
		// otherwise produce an unbroken stream of skipped runs against a
		// team that can never be revived without an explicit un-archive.
		// Pair with TransferTeamAutomationsToLeader on DeleteTeam so any
		// automation that survives the archive flips to executor_type='agent'
		// (the leader) and stops referencing the dead team row.
		if team.ArchivedAt.Valid {
			writeError(w, http.StatusUnprocessableEntity, "team is archived; pick a different team")
			return false
		}
		leader, err := q.LockAgentForAutomationAssignment(r.Context(), db.LockAgentForAutomationAssignmentParams{
			ID:          team.LeaderID,
			WorkspaceID: workspaceID,
		})
		if err != nil {
			writeError(w, http.StatusBadRequest, "team leader agent not found")
			return false
		}
		if leader.ArchivedAt.Valid {
			writeError(w, http.StatusUnprocessableEntity, "team leader is archived; pick a different team or rotate the leader before assigning automation")
			return false
		}
		if requireRuntime && !leader.RuntimeID.Valid {
			writeError(w, http.StatusUnprocessableEntity, "team leader needs a runtime before this automation can be active")
			return false
		}
		// Private-leader gate: the member configuring the automation must have
		// access to the private leader, same as validateAssigneePair.
		actorType, actorID := h.resolveActor(r, requestUserID(r), util.UUIDToString(workspaceID))
		if !h.canInvokeAgent(r.Context(), leader, actorType, actorID, h.invokeOriginatorFromRequest(r, actorType, actorID), util.UUIDToString(workspaceID)) {
			writeError(w, http.StatusForbidden, "cannot assign automation to team with private leader")
			return false
		}
		return true
	default:
		writeError(w, http.StatusBadRequest, "executor_type must be agent or team")
		return false
	}
}

func (h *Handler) UpdateAutomationTrigger(w http.ResponseWriter, r *http.Request) {
	automationID := chi.URLParam(r, "id")
	triggerID := chi.URLParam(r, "triggerId")
	workspaceID := h.resolveWorkspaceID(r)

	ap, ok := h.loadAutomationInWorkspace(w, r, automationID, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationWrite(w, r, ap, workspaceID) {
		return
	}

	triggerUUID, ok := parseUUIDOrBadRequest(w, triggerID, "trigger id")
	if !ok {
		return
	}

	prev, err := h.Queries.GetAutomationTrigger(r.Context(), triggerUUID)
	if err != nil || uuidToString(prev.AutomationID) != uuidToString(ap.ID) {
		writeError(w, http.StatusNotFound, "trigger not found")
		return
	}

	var req UpdateAutomationTriggerRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}

	// Kind-specific validation. Mirrors the create-path discipline: cron
	// and timezone only make sense on schedule triggers, so reject loudly
	// rather than persisting fields that no code path reads. enabled and
	// label remain valid on every kind.
	if prev.Kind != "schedule" {
		if req.CronExpression != nil {
			writeError(w, http.StatusBadRequest, "cron_expression is only valid for schedule triggers")
			return
		}
		if req.Timezone != nil {
			writeError(w, http.StatusBadRequest, "timezone is only valid for schedule triggers")
			return
		}
	}

	params := db.UpdateAutomationTriggerParams{
		ID:             prev.ID,
		CronExpression: prev.CronExpression,
		Timezone:       prev.Timezone,
		NextRunAt:      prev.NextRunAt,
		Label:          prev.Label,
	}
	if req.Enabled != nil {
		params.Enabled = pgtype.Bool{Bool: *req.Enabled, Valid: true}
	}
	if req.CronExpression != nil {
		params.CronExpression = pgtype.Text{String: *req.CronExpression, Valid: true}
	}
	if req.Timezone != nil {
		if *req.Timezone != "" {
			if err := service.ValidateTimezone(*req.Timezone); err != nil {
				writeError(w, http.StatusBadRequest, err.Error())
				return
			}
		}
		params.Timezone = pgtype.Text{String: *req.Timezone, Valid: true}
	}
	if req.Label != nil {
		params.Label = pgtype.Text{String: *req.Label, Valid: true}
	}
	// Tri-state PATCH for event_filters. A nil pointer (field omitted or
	// JSON null) leaves the existing row untouched — params.EventFilters
	// stays unset and the COALESCE in the UPDATE preserves the previous
	// value. A non-nil pointer is authoritative: an empty slice clears
	// filters (encoded as the JSONB literal `[]` so COALESCE replaces
	// rather than preserves), a populated slice replaces.
	if req.EventFilters != nil {
		if prev.Kind != "webhook" {
			writeError(w, http.StatusBadRequest, "event_filters is only valid for webhook triggers")
			return
		}
		if err := validateWebhookEventFilters(*req.EventFilters); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		encoded, err := encodeWebhookEventFiltersAlways(*req.EventFilters)
		if err != nil {
			writeError(w, http.StatusInternalServerError, "failed to encode event_filters")
			return
		}
		params.EventFilters = encoded
	}

	// Recompute next_run_at if cron or timezone changed.
	cronExpr := prev.CronExpression.String
	if req.CronExpression != nil {
		cronExpr = *req.CronExpression
	}
	tz := "UTC"
	if prev.Timezone.Valid {
		tz = prev.Timezone.String
	}
	if req.Timezone != nil {
		tz = *req.Timezone
	}
	if prev.Kind == "schedule" && cronExpr != "" {
		t, err := computeNextRun(cronExpr, tz)
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		params.NextRunAt = pgtype.Timestamptz{Time: t, Valid: true}
	}

	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}

	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update trigger")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	trigger, err := qtx.UpdateAutomationTrigger(r.Context(), params)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update trigger")
		return
	}

	// Only a substantive edit republishes the rule version and transfers this
	// trigger's accountability to the editor. cron / timezone / enabled / event_filters
	// change WHAT or WHEN the trigger fires; label is a cosmetic display field, and a
	// no-op PATCH changes nothing — neither should move responsibility (MUL-4302; the
	// over-transfer Elon flagged). Comparing the persisted before/after rows captures a
	// real change and ignores label-only / no-op PATCHes (next_run_at is derived from
	// cron/timezone, so it is not an independent signal).
	triggerSubstantiveChange := prev.Enabled != trigger.Enabled ||
		prev.CronExpression != trigger.CronExpression ||
		prev.Timezone != trigger.Timezone ||
		!bytes.Equal(prev.EventFilters, trigger.EventFilters)
	if triggerSubstantiveChange {
		if err := h.recordAutomationRuleVersion(r.Context(), qtx, ap, "member", parseUUID(userID)); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to update trigger")
			return
		}
		// Responsibility for THIS trigger's runs transfers to the editor. Scoped to the
		// single row so editing one trigger never reassigns another's accountability —
		// the per-firing-trigger granularity the automation-scoped rule_version can't give.
		if err := qtx.SetAutomationTriggerPublisher(r.Context(), db.SetAutomationTriggerPublisherParams{
			ID:              trigger.ID,
			PublishedByType: pgtype.Text{String: "member", Valid: true},
			PublishedByID:   parseUUID(userID),
		}); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to update trigger")
			return
		}
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update trigger")
		return
	}

	resp := h.triggerToResponse(trigger)
	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", userID, map[string]any{
		"automation_id": uuidToString(ap.ID),
		"trigger":      resp,
	})
	writeJSON(w, http.StatusOK, resp)
}

func (h *Handler) DeleteAutomationTrigger(w http.ResponseWriter, r *http.Request) {
	automationID := chi.URLParam(r, "id")
	triggerID := chi.URLParam(r, "triggerId")
	workspaceID := h.resolveWorkspaceID(r)

	automationUUID, ok := parseUUIDOrBadRequest(w, automationID, "automation id")
	if !ok {
		return
	}
	triggerUUID, ok := parseUUIDOrBadRequest(w, triggerID, "trigger id")
	if !ok {
		return
	}
	wsUUID, ok := parseUUIDOrBadRequest(w, workspaceID, "workspace id")
	if !ok {
		return
	}

	ap, err := h.Queries.GetAutomationInWorkspace(r.Context(), db.GetAutomationInWorkspaceParams{
		ID:          automationUUID,
		WorkspaceID: wsUUID,
	})
	if err != nil {
		writeError(w, http.StatusNotFound, "automation not found")
		return
	}
	if !h.requireAutomationWrite(w, r, ap, workspaceID) {
		return
	}

	trigger, err := h.Queries.GetAutomationTrigger(r.Context(), triggerUUID)
	if err != nil || uuidToString(trigger.AutomationID) != uuidToString(automationUUID) {
		writeError(w, http.StatusNotFound, "trigger not found")
		return
	}

	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}

	// Removing a trigger changes what fires — a substantive publish (MUL-4302 §3.4).
	// Republish the rule version with this member as publisher, atomically with the
	// delete.
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete trigger")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)

	if err := qtx.DeleteAutomationTrigger(r.Context(), triggerUUID); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete trigger")
		return
	}
	if err := h.recordAutomationRuleVersion(r.Context(), qtx, ap, "member", parseUUID(userID)); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete trigger")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to delete trigger")
		return
	}

	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", userID, map[string]any{
		"automation_id": uuidToString(automationUUID),
		"trigger_id":   uuidToString(triggerUUID),
	})
	w.WriteHeader(http.StatusNoContent)
}

// RotateAutomationTriggerWebhookToken issues a fresh bearer token for an
// existing webhook trigger. The old token stops working immediately because
// the unique-index lookup in the public ingress route is keyed on the
// current row value.
func (h *Handler) RotateAutomationTriggerWebhookToken(w http.ResponseWriter, r *http.Request) {
	automationID := chi.URLParam(r, "id")
	triggerID := chi.URLParam(r, "triggerId")
	workspaceID := h.resolveWorkspaceID(r)

	ap, ok := h.loadAutomationInWorkspace(w, r, automationID, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationWrite(w, r, ap, workspaceID) {
		return
	}

	triggerUUID, ok := parseUUIDOrBadRequest(w, triggerID, "trigger id")
	if !ok {
		return
	}
	prev, err := h.Queries.GetAutomationTrigger(r.Context(), triggerUUID)
	if err != nil || uuidToString(prev.AutomationID) != uuidToString(ap.ID) {
		writeError(w, http.StatusNotFound, "trigger not found")
		return
	}
	if prev.Kind != "webhook" {
		writeError(w, http.StatusBadRequest, "trigger is not a webhook trigger")
		return
	}

	var rotated db.AutomationTrigger
	for attempt := 0; attempt < 3; attempt++ {
		token, terr := generateWebhookToken()
		if terr != nil {
			writeError(w, http.StatusInternalServerError, "failed to generate webhook token")
			return
		}
		rotated, err = h.Queries.RotateAutomationTriggerWebhookToken(r.Context(), db.RotateAutomationTriggerWebhookTokenParams{
			ID:           triggerUUID,
			WebhookToken: pgtype.Text{String: token, Valid: true},
		})
		if err == nil {
			break
		}
		if !isUniqueViolation(err) {
			writeError(w, http.StatusInternalServerError, "failed to rotate webhook token")
			return
		}
	}
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to rotate webhook token")
		return
	}

	resp := h.triggerToResponse(rotated)
	userID, _ := requireUserID(w, r)
	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", userID, map[string]any{
		"automation_id": uuidToString(ap.ID),
		"trigger":      resp,
	})
	writeJSON(w, http.StatusOK, resp)
}

// SetAutomationTriggerSigningSecret sets (or clears) the HMAC signing secret
// for a webhook trigger. Lives on its own endpoint so the secret value never
// shares a request body with any other field — keeping it out of generic
// request-body logs and audit captures that may include patch payloads.
//
// Empty body / empty `signing_secret` clears the secret and reverts the
// trigger to bearer-token-only authentication. The response carries
// `has_signing_secret` + `signing_secret_hint`; the secret itself is never
// echoed back, matching the GitHub / Stripe industry pattern.
func (h *Handler) SetAutomationTriggerSigningSecret(w http.ResponseWriter, r *http.Request) {
	automationID := chi.URLParam(r, "id")
	triggerID := chi.URLParam(r, "triggerId")
	workspaceID := h.resolveWorkspaceID(r)

	ap, ok := h.loadAutomationInWorkspace(w, r, automationID, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationWrite(w, r, ap, workspaceID) {
		return
	}
	triggerUUID, ok := parseUUIDOrBadRequest(w, triggerID, "trigger id")
	if !ok {
		return
	}
	prev, err := h.Queries.GetAutomationTrigger(r.Context(), triggerUUID)
	if err != nil || uuidToString(prev.AutomationID) != uuidToString(ap.ID) {
		writeError(w, http.StatusNotFound, "trigger not found")
		return
	}
	if prev.Kind != "webhook" {
		writeError(w, http.StatusBadRequest, "trigger is not a webhook trigger")
		return
	}

	var req SetSigningSecretRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	secret := strings.TrimSpace(req.SigningSecret)
	// 16 chars is the floor: enough to make brute force impractical for the
	// SHA-256 HMAC but low enough not to reject providers that mint shorter
	// keys (Slack signing secrets are 32 hex chars; GitHub recommends 32).
	if secret != "" && len(secret) < 16 {
		writeError(w, http.StatusBadRequest, "signing_secret must be at least 16 characters")
		return
	}

	param := db.SetAutomationTriggerSigningSecretParams{ID: triggerUUID}
	if secret != "" {
		param.SigningSecret = pgtype.Text{String: secret, Valid: true}
	}
	updated, err := h.Queries.SetAutomationTriggerSigningSecret(r.Context(), param)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update signing secret")
		return
	}

	resp := h.triggerToResponse(updated)
	userID, _ := requireUserID(w, r)
	// Publish the trigger update so the UI can refresh the has_signing_secret
	// badge in real time. The event payload only carries the response shape,
	// which excludes the secret.
	h.publish(protocol.EventAutomationUpdated, workspaceID, "member", userID, map[string]any{
		"automation_id": uuidToString(ap.ID),
		"trigger":      resp,
	})
	writeJSON(w, http.StatusOK, resp)
}

// ── Runs ────────────────────────────────────────────────────────────────────

func (h *Handler) ListAutomationRuns(w http.ResponseWriter, r *http.Request) {
	automationID := chi.URLParam(r, "id")
	workspaceID := h.resolveWorkspaceID(r)

	automation, ok := h.loadAutomationInWorkspace(w, r, automationID, workspaceID)
	if !ok {
		return
	}

	limit := int32(20)
	offset := int32(0)
	if l := r.URL.Query().Get("limit"); l != "" {
		if v, err := strconv.Atoi(l); err == nil && v > 0 {
			limit = int32(v)
		}
	}
	if limit > 100 {
		limit = 100
	}
	if o := r.URL.Query().Get("offset"); o != "" {
		if v, err := strconv.Atoi(o); err == nil && v >= 0 {
			offset = int32(v)
		}
	}

	runs, err := h.Queries.ListAutomationRuns(r.Context(), db.ListAutomationRunsParams{
		AutomationID: automation.ID,
		Limit:       limit,
		Offset:      offset,
	})
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list runs")
		return
	}

	resp := make([]AutomationRunResponse, len(runs))
	for i, run := range runs {
		// Omit trigger_payload in the list response — a webhook envelope
		// can be up to 256 KiB and `limit` defaults to 20, so the full
		// list would be a ~5 MB worst case. Detail dialog fetches the
		// full payload from GetAutomationRun.
		resp[i] = runToResponseSlim(run)
	}
	writeJSON(w, http.StatusOK, map[string]any{"runs": resp, "total": len(resp)})
}

// GetAutomationRun returns a single run including its full trigger_payload.
// Workspace scoping is enforced via loadAutomationInWorkspace; the run is
// then re-checked to belong to that automation so a guessed runId from
// another workspace cannot leak data.
func (h *Handler) GetAutomationRun(w http.ResponseWriter, r *http.Request) {
	automationID := chi.URLParam(r, "id")
	runID := chi.URLParam(r, "runId")
	workspaceID := h.resolveWorkspaceID(r)

	automation, ok := h.loadAutomationInWorkspace(w, r, automationID, workspaceID)
	if !ok {
		return
	}

	runUUID, ok := parseUUIDOrBadRequest(w, runID, "run id")
	if !ok {
		return
	}

	run, err := h.Queries.GetAutomationRun(r.Context(), runUUID)
	if err != nil {
		writeError(w, http.StatusNotFound, "run not found")
		return
	}
	if uuidToString(run.AutomationID) != uuidToString(automation.ID) {
		// Guard against a runId from another automation being requested via
		// this automation's URL — fail closed with 404 so the response shape
		// matches the "not found" case and no information is leaked.
		writeError(w, http.StatusNotFound, "run not found")
		return
	}

	writeJSON(w, http.StatusOK, runToResponse(run))
}

// ── Manual trigger ──────────────────────────────────────────────────────────

func (h *Handler) TriggerAutomation(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	workspaceID := h.resolveWorkspaceID(r)

	automation, ok := h.loadAutomationInWorkspace(w, r, id, workspaceID)
	if !ok {
		return
	}
	if !h.requireAutomationWrite(w, r, automation, workspaceID) {
		return
	}
	if automation.Status != "active" {
		writeError(w, http.StatusBadRequest, "automation is not active")
		return
	}

	// A manual "run now" is a direct human action, so the run is attributed
	// direct_human to the triggering member (MUL-4302 §4). Resolve the actor the
	// same way assign/promote does; only a member actor is a human — an agent
	// triggering via A2A yields an invalid actor and falls back to rule_owner.
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	actorType, actorID := h.resolveActor(r, userID, workspaceID)

	idempotencyKey := strings.TrimSpace(r.Header.Get("Idempotency-Key"))
	if len(idempotencyKey) > 255 {
		writeError(w, http.StatusBadRequest, "Idempotency-Key is too long")
		return
	}
	if idempotencyKey == "" {
		idempotencyKey = service.NewRequestIdempotencyKey()
	}
	run, reasonCode, err := h.AutomationService.DispatchAutomationManualWithKey(r.Context(), automation, pgtype.UUID{}, nil, memberActorUserID(actorType, actorID), idempotencyKey)
	if err != nil {
		var quotaErr *service.AutomationQuotaExceededError
		if errors.As(err, &quotaErr) {
			retryAfter := int64(time.Until(quotaErr.ResetAt).Seconds())
			if retryAfter < 1 {
				retryAfter = 1
			}
			w.Header().Set("Retry-After", strconv.FormatInt(retryAfter, 10))
			writeJSON(w, http.StatusTooManyRequests, map[string]any{
				"reason_code": "quota_exceeded", "used": quotaErr.Used,
				"reserved": quotaErr.Reserved, "limit": quotaErr.Limit,
				"reset_at": quotaErr.ResetAt.UTC().Format(time.RFC3339),
			})
			return
		}
		// Everything past the quota branch is an unclassified internal failure
		// whose chain carries pgx constraint/table names and internal ids. Any
		// workspace member can reach "run now", so the detail stays in the log
		// and the response is the same fixed 5xx string the rest of this file
		// returns (MUL-6472).
		slog.Error("trigger automation failed",
			"error", err,
			"automation_id", uuidToString(automation.ID),
		)
		writeError(w, http.StatusInternalServerError, "failed to trigger automation")
		return
	}

	// Carry the typed admission reason (decided at its source, MUL-4525) straight
	// into the response — no reverse-engineering from failure_reason text. The
	// UI branches on run status + this code for the "run now" toast.
	resp := runToResponse(*run)
	if reasonCode != "" {
		c := string(reasonCode)
		resp.ReasonCode = &c
	}
	writeJSON(w, http.StatusOK, resp)
}

// GetAutomationQuotaUsage exposes Cloud-provided interval facts plus durable
// server-owned blocked counts. When the gate is off or malformed, the service
// returns before any quota-table read.
func (h *Handler) GetAutomationQuotaUsage(w http.ResponseWriter, r *http.Request) {
	workspaceID, err := util.ParseUUID(h.resolveWorkspaceID(r))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid workspace")
		return
	}
	usage, err := h.AutomationService.AutomationQuotaUsage(r.Context(), workspaceID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load automation quota usage")
		return
	}
	resp := AutomationQuotaUsageResponse{Action: "off"}
	if usage.Enabled {
		resp.Action = usage.Action
		resp.Used, resp.Reserved, resp.Total = usage.Used, usage.Reserved, usage.Total
		resp.Limit, resp.Reached = usage.Limit, usage.Reached
		resp.BlockedCounts = usage.BlockedCounts
		if usage.PeriodStart != nil {
			v := usage.PeriodStart.UTC().Format(time.RFC3339)
			resp.PeriodStart = &v
		}
		if usage.PeriodEnd != nil {
			v := usage.PeriodEnd.UTC().Format(time.RFC3339)
			resp.PeriodEnd = &v
		}
		if usage.ResetAt != nil {
			v := usage.ResetAt.UTC().Format(time.RFC3339)
			resp.ResetAt = &v
		}
	}
	writeJSON(w, http.StatusOK, resp)
}
