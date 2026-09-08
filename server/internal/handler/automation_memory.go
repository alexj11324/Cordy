package handler

import (
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5"
	"github.com/orvilo-ai/orvilo/server/internal/service"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

var automationMemoryName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*\.md$`)

type automationMemoryResponse struct {
	Name      string    `json:"name"`
	Content   string    `json:"content"`
	Revision  int64     `json:"revision"`
	UpdatedAt time.Time `json:"updated_at"`
}

func memoryResponse(m db.AutomationMemory) automationMemoryResponse {
	return automationMemoryResponse{m.Name, m.Content, m.Revision, m.UpdatedAt.Time}
}

// A run token gets access only to its own automation, while it is running and
// memories are enabled. Human access follows the existing automation write gate.
func authorizeAutomationMemory(w http.ResponseWriter, r *http.Request, ap db.Automation, queries *db.Queries) bool {
	if r.Header.Get("X-Actor-Source") != "task_token" {
		if r.Header.Get("X-Actor-Source") == "cloud_pat" || r.Header.Get("X-Agent-ID") != "" {
			writeError(w, http.StatusForbidden, "automation memories require a member or bound automation task")
			return false
		}
		userID, ok := requireUserID(w, r)
		if !ok {
			return false
		}
		member, err := queries.GetMemberByUserAndWorkspace(r.Context(), db.GetMemberByUserAndWorkspaceParams{UserID: parseUUID(userID), WorkspaceID: ap.WorkspaceID})
		if err != nil {
			writeError(w, http.StatusNotFound, "workspace not found")
			return false
		}
		if automationWriteByOwnership(ap, member) {
			return true
		}
		granted, err := queries.IsAutomationCollaborator(r.Context(), db.IsAutomationCollaboratorParams{AutomationID: ap.ID, UserID: member.UserID})
		if err == nil && granted {
			return true
		}
		writeError(w, http.StatusForbidden, "only the automation creator, a workspace admin, or a granted collaborator can manage memories")
		return false
	}
	cfg := service.ParseAutomationTools(ap.Tools)
	if cfg.Memories == nil {
		writeError(w, http.StatusForbidden, "memories are not configured for this automation")
		return false
	}
	taskID := parseUUID(r.Header.Get("X-Task-ID"))
	task, err := queries.GetAgentTaskInWorkspace(r.Context(), db.GetAgentTaskInWorkspaceParams{ID: taskID, WorkspaceID: ap.WorkspaceID})
	if err == nil && task.Status == "running" && uuidToString(task.AgentID) == r.Header.Get("X-Agent-ID") {
		if task.TriggerEvidenceKind.String == "agent_thread_continuation" {
			// A follow-up is not another automation run. Its server-owned thread
			// lineage retains capabilities only while its human requester still
			// has permission to execute this automation. Shared Agent invocation
			// alone must not grant access to another member's private memories.
			member, err := queries.GetMemberByUserAndWorkspace(r.Context(), db.GetMemberByUserAndWorkspaceParams{
				UserID: task.OriginatorUserID, WorkspaceID: ap.WorkspaceID,
			})
			if err != nil {
				writeError(w, http.StatusForbidden, "continuation requester cannot access automation memories")
				return false
			}
			if !automationWriteByOwnership(ap, member) {
				granted, err := queries.IsAutomationCollaborator(r.Context(), db.IsAutomationCollaboratorParams{AutomationID: ap.ID, UserID: member.UserID})
				if err != nil || !granted {
					writeError(w, http.StatusForbidden, "continuation requester cannot access automation memories")
					return false
				}
			}
		}
		if task.AutomationRunID.Valid {
			run, err := queries.GetAutomationRun(r.Context(), task.AutomationRunID)
			if err == nil && run.AutomationID == ap.ID {
				return true
			}
		} else if task.IssueID.Valid {
			issue, err := queries.GetIssue(r.Context(), task.IssueID)
			if err == nil && issue.WorkspaceID == ap.WorkspaceID && issue.OriginType.String == "automation" && issue.OriginID == ap.ID {
				return true
			}
		} else if task.TriggerEvidenceKind.String == "agent_thread_continuation" {
			thread, err := queries.ListAgentThreadTasks(r.Context(), task.ID)
			if err == nil {
				if runID, ok := service.AgentThreadAutomationRunID(thread); ok {
					run, err := queries.GetAutomationRun(r.Context(), runID)
					if err == nil && run.AutomationID == ap.ID {
						return true
					}
				}
			}
		}
	}
	writeError(w, http.StatusForbidden, "task is not running for this automation")
	return false
}

func (h *Handler) loadAutomationMemoryScope(w http.ResponseWriter, r *http.Request) (db.Automation, bool) {
	ap, ok := h.loadAutomationInWorkspace(w, r, chi.URLParam(r, "id"), h.resolveWorkspaceID(r))
	if !ok || !authorizeAutomationMemory(w, r, ap, h.Queries) {
		return db.Automation{}, false
	}
	return ap, true
}

func validAutomationMemoryName(w http.ResponseWriter, r *http.Request) (string, bool) {
	name := chi.URLParam(r, "name")
	if len(name) > 128 || !automationMemoryName.MatchString(name) {
		writeError(w, http.StatusBadRequest, "memory name must be a Markdown basename of at most 128 bytes")
		return "", false
	}
	return name, true
}

func (h *Handler) ListAutomationMemories(w http.ResponseWriter, r *http.Request) {
	ap, ok := h.loadAutomationMemoryScope(w, r)
	if !ok {
		return
	}
	items, err := h.Queries.ListAutomationMemories(r.Context(), ap.ID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to list memories")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"items": items})
}

func (h *Handler) GetAutomationMemory(w http.ResponseWriter, r *http.Request) {
	ap, ok := h.loadAutomationMemoryScope(w, r)
	if !ok {
		return
	}
	name, ok := validAutomationMemoryName(w, r)
	if !ok {
		return
	}
	m, err := h.Queries.GetAutomationMemory(r.Context(), db.GetAutomationMemoryParams{AutomationID: ap.ID, Name: name})
	if errors.Is(err, pgx.ErrNoRows) || (err == nil && m.Deleted) {
		writeError(w, http.StatusNotFound, "memory not found")
		return
	}
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to read memory")
		return
	}
	writeJSON(w, http.StatusOK, memoryResponse(m))
}

func (h *Handler) PutAutomationMemory(w http.ResponseWriter, r *http.Request) {
	h.mutateAutomationMemory(w, r, false)
}

func (h *Handler) DeleteAutomationMemory(w http.ResponseWriter, r *http.Request) {
	h.mutateAutomationMemory(w, r, true)
}

func (h *Handler) mutateAutomationMemory(w http.ResponseWriter, r *http.Request, remove bool) {
	ap, ok := h.loadAutomationMemoryScope(w, r)
	if !ok {
		return
	}
	name, ok := validAutomationMemoryName(w, r)
	if !ok {
		return
	}
	var content string
	var revision int64
	if remove {
		var err error
		revision, err = strconv.ParseInt(r.URL.Query().Get("revision"), 10, 64)
		if err != nil || revision < 1 {
			writeError(w, http.StatusBadRequest, "revision is required")
			return
		}
	} else {
		var req struct {
			Content          *string `json:"content"`
			ExpectedRevision *int64  `json:"expected_revision"`
		}
		if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 512*1024)).Decode(&req); err != nil || req.Content == nil || req.ExpectedRevision == nil || *req.ExpectedRevision < 0 {
			writeError(w, http.StatusBadRequest, "content and nonnegative expected_revision are required")
			return
		}
		content, revision = *req.Content, *req.ExpectedRevision
		if len(content) > 65536 || !utf8.ValidString(content) || strings.ContainsRune(content, 0) {
			writeError(w, http.StatusBadRequest, "memory content must be UTF-8 and at most 64 KiB")
			return
		}
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to save memory")
		return
	}
	defer tx.Rollback(r.Context())
	qtx := h.Queries.WithTx(tx)
	if _, err = qtx.LockAutomationMemoryWorkspace(r.Context(), ap.WorkspaceID); err != nil {
		writeError(w, http.StatusNotFound, "workspace not found")
		return
	}
	ap, err = qtx.LockAutomationForUpdate(r.Context(), db.LockAutomationForUpdateParams{ID: ap.ID, WorkspaceID: ap.WorkspaceID})
	if err != nil {
		writeError(w, http.StatusNotFound, "automation not found")
		return
	}
	if !authorizeAutomationMemory(w, r, ap, qtx) {
		return
	}
	current, err := qtx.GetAutomationMemory(r.Context(), db.GetAutomationMemoryParams{AutomationID: ap.ID, Name: name})
	if err != nil && !errors.Is(err, pgx.ErrNoRows) {
		slog.Error("automation memory lookup failed", "automation_id", uuidToString(ap.ID), "error", err)
		writeError(w, http.StatusInternalServerError, "failed to read memory")
		return
	}
	exists := err == nil && !current.Deleted
	if (exists && revision != current.Revision) || (!exists && (revision != 0 || remove)) {
		writeError(w, http.StatusConflict, "memory changed; reload before saving your edits")
		return
	}
	var saved db.AutomationMemory
	if remove {
		err = qtx.DeleteAutomationMemory(r.Context(), db.DeleteAutomationMemoryParams{AutomationID: ap.ID, Name: name})
	} else {
		saved, err = qtx.PutAutomationMemory(r.Context(), db.PutAutomationMemoryParams{AutomationID: ap.ID, Name: name, Content: content})
	}
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to save memory")
		return
	}
	if err = tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to save memory")
		return
	}
	if remove {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	writeJSON(w, http.StatusOK, memoryResponse(saved))
}
