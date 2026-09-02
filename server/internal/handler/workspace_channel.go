package handler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"strings"
	"unicode"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
)

const (
	workspaceChannelNameMaxRunes        = 80
	workspaceChannelDescriptionMaxRunes = 240
	workspaceChannelSlugMaxRunes        = 64
	workspaceChannelMessageMaxRunes     = 20_000

	workspaceChannelCreatedEvent = "channel:created"
	workspaceChannelMessageEvent = "channel:message"
)

// These statements mirror server/pkg/db/queries/workspace_channel.sql. The
// generated wrapper is intentionally not edited in this phase; keeping the
// handler on explicit schema-correct statements lets the branch compile until
// the owner regenerates sqlc.
const getWorkspaceChannelStatement = `
SELECT id, workspace_id, name, slug, description, created_by, archived_at, created_at, updated_at
FROM workspace_channel
WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL
`

const listWorkspaceChannelsStatement = `
SELECT id, workspace_id, name, slug, description, created_by, archived_at, created_at, updated_at
FROM workspace_channel
WHERE workspace_id = $1 AND archived_at IS NULL
ORDER BY created_at, id
`

const listWorkspaceChannelMessagesStatement = `
SELECT id, workspace_id, channel_id, author_type, author_id, content,
       parent_id, quoted_message_id, created_at, updated_at
FROM workspace_channel_message AS message
WHERE message.workspace_id = $1
  AND message.channel_id = $2
  AND EXISTS (
      SELECT 1
      FROM workspace_channel AS channel
      WHERE channel.id = message.channel_id
        AND channel.workspace_id = message.workspace_id
        AND channel.archived_at IS NULL
  )
  AND ($3::timestamptz IS NULL OR (message.created_at, message.id) < ($3::timestamptz, $4::uuid))
ORDER BY message.created_at DESC, message.id DESC
LIMIT $5
`

const getWorkspaceChannelMessageStatement = `
SELECT id, workspace_id, channel_id, author_type, author_id, content,
       parent_id, quoted_message_id, created_at, updated_at
FROM workspace_channel_message AS message
WHERE message.id = $1
  AND message.workspace_id = $2
  AND EXISTS (
      SELECT 1
      FROM workspace_channel AS channel
      WHERE channel.id = message.channel_id
        AND channel.workspace_id = message.workspace_id
        AND channel.archived_at IS NULL
  )
`

const createWorkspaceChannelStatement = `
INSERT INTO workspace_channel (workspace_id, slug, name, description, created_by)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, workspace_id, name, slug, description, created_by, archived_at, created_at, updated_at
`

const createWorkspaceChannelMessageStatement = `
WITH channel AS MATERIALIZED (
	SELECT id
	FROM workspace_channel
	WHERE id = $2 AND workspace_id = $1 AND archived_at IS NULL
), valid_author AS MATERIALIZED (
	SELECT 1
	WHERE (
		($3 = 'member' AND EXISTS (
			SELECT 1 FROM member WHERE workspace_id = $1 AND user_id = $4
		))
		OR ($3 = 'agent' AND EXISTS (
			SELECT 1 FROM agent WHERE workspace_id = $1 AND id = $4
		))
	)
), valid_parent AS MATERIALIZED (
	SELECT 1
	WHERE $6::uuid IS NULL OR EXISTS (
		SELECT 1
		FROM workspace_channel_message
		WHERE id = $6 AND workspace_id = $1 AND channel_id = $2
	)
), valid_quote AS MATERIALIZED (
	SELECT 1
	WHERE $7::uuid IS NULL OR EXISTS (
		SELECT 1
		FROM workspace_channel_message
		WHERE id = $7 AND workspace_id = $1 AND channel_id = $2
	)
)
INSERT INTO workspace_channel_message (
	workspace_id, channel_id, author_type, author_id, content,
	parent_id, quoted_message_id
)
SELECT $1, $2, $3, $4, $5, $6, $7
FROM channel, valid_author, valid_parent, valid_quote
RETURNING id, workspace_id, channel_id, author_type, author_id, content,
	parent_id, quoted_message_id, created_at, updated_at
`

func scanWorkspaceChannel(row pgx.Row) (db.WorkspaceChannel, error) {
	var channel db.WorkspaceChannel
	err := row.Scan(
		&channel.ID,
		&channel.WorkspaceID,
		&channel.Name,
		&channel.Slug,
		&channel.Description,
		&channel.CreatedBy,
		&channel.ArchivedAt,
		&channel.CreatedAt,
		&channel.UpdatedAt,
	)
	return channel, err
}

func scanWorkspaceChannelMessage(row pgx.Row) (db.WorkspaceChannelMessage, error) {
	var message db.WorkspaceChannelMessage
	err := row.Scan(
		&message.ID,
		&message.WorkspaceID,
		&message.ChannelID,
		&message.AuthorType,
		&message.AuthorID,
		&message.Content,
		&message.ParentID,
		&message.QuotedMessageID,
		&message.CreatedAt,
		&message.UpdatedAt,
	)
	return message, err
}

type workspaceChannelMessagesResponse struct {
	Messages   []db.WorkspaceChannelMessage `json:"messages"`
	Limit      int                          `json:"limit"`
	HasMore    bool                         `json:"has_more"`
	NextCursor *ChatMessagesCursorResponse  `json:"next_cursor,omitempty"`
}

func cleanWorkspaceChannelText(value string) string {
	return strings.TrimSpace(strings.ReplaceAll(value, "\x00", ""))
}

func workspaceChannelTextWithinLimit(value string, maxRunes int) bool {
	return len([]rune(value)) <= maxRunes
}

func slugifyWorkspaceChannelName(value string) string {
	var slug strings.Builder
	slugRunes := 0
	separatorPending := false
	for _, r := range value {
		if unicode.IsLetter(r) || unicode.IsNumber(r) {
			if separatorPending && slugRunes > 0 {
				// A separator needs one more rune after it. Stop before
				// writing it when truncation would otherwise leave a trailing
				// hyphen in the derived slug.
				if slugRunes+1 >= workspaceChannelSlugMaxRunes {
					break
				}
				slug.WriteByte('-')
				slugRunes++
			}
			if slugRunes >= workspaceChannelSlugMaxRunes {
				break
			}
			slug.WriteRune(unicode.ToLower(r))
			slugRunes++
			separatorPending = false
			continue
		}
		if slugRunes > 0 {
			separatorPending = true
		}
	}
	return slug.String()
}

func normalizeWorkspaceChannelSlug(value string) (string, bool) {
	value = cleanWorkspaceChannelText(value)
	if value == "" || !workspaceChannelTextWithinLimit(value, workspaceChannelSlugMaxRunes) {
		return "", false
	}

	var normalized strings.Builder
	lastWasHyphen := false
	for _, r := range value {
		switch {
		case unicode.IsLetter(r) || unicode.IsNumber(r):
			normalized.WriteRune(unicode.ToLower(r))
			lastWasHyphen = false
		case r == '-' && normalized.Len() > 0 && !lastWasHyphen:
			normalized.WriteByte('-')
			lastWasHyphen = true
		default:
			return "", false
		}
	}
	result := normalized.String()
	if result == "" || strings.HasSuffix(result, "-") {
		return "", false
	}
	return result, true
}

func scanWorkspaceChannels(rows pgx.Rows) ([]db.WorkspaceChannel, error) {
	defer rows.Close()
	channels := make([]db.WorkspaceChannel, 0)
	for rows.Next() {
		var channel db.WorkspaceChannel
		if err := rows.Scan(
			&channel.ID,
			&channel.WorkspaceID,
			&channel.Name,
			&channel.Slug,
			&channel.Description,
			&channel.CreatedBy,
			&channel.ArchivedAt,
			&channel.CreatedAt,
			&channel.UpdatedAt,
		); err != nil {
			return nil, err
		}
		channels = append(channels, channel)
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	return channels, nil
}

func scanWorkspaceChannelMessages(rows pgx.Rows) ([]db.WorkspaceChannelMessage, error) {
	defer rows.Close()
	messages := make([]db.WorkspaceChannelMessage, 0)
	for rows.Next() {
		var message db.WorkspaceChannelMessage
		if err := rows.Scan(
			&message.ID,
			&message.WorkspaceID,
			&message.ChannelID,
			&message.AuthorType,
			&message.AuthorID,
			&message.Content,
			&message.ParentID,
			&message.QuotedMessageID,
			&message.CreatedAt,
			&message.UpdatedAt,
		); err != nil {
			return nil, err
		}
		messages = append(messages, message)
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	return messages, nil
}

func (h *Handler) workspaceChannelDependencies(w http.ResponseWriter) bool {
	if h == nil || h.Queries == nil || h.DB == nil {
		writeError(w, http.StatusServiceUnavailable, "database unavailable")
		return false
	}
	return true
}

func workspaceChannelMemberMatchesWorkspace(member db.Member, workspaceID string) bool {
	return member.WorkspaceID.Valid && uuidToString(member.WorkspaceID) == workspaceID
}

func (h *Handler) publishWorkspaceChannelEvent(eventType, workspaceID, actorType, actorID string, payload any) {
	if h == nil || h.Bus == nil {
		return
	}
	h.publish(eventType, workspaceID, actorType, actorID, payload)
}

func (h *Handler) workspaceChannelMessageByID(ctx context.Context, messageID, workspaceID pgtype.UUID) (db.WorkspaceChannelMessage, error) {
	if h == nil || h.DB == nil {
		return db.WorkspaceChannelMessage{}, errors.New("database unavailable")
	}
	return scanWorkspaceChannelMessage(h.DB.QueryRow(ctx, getWorkspaceChannelMessageStatement, messageID, workspaceID))
}

// dispatchWorkspaceChannelMentions is deliberately best-effort. A channel
// message is already durable and visible when this bridge runs; a bad or
// unavailable agent must not turn a successful channel send into a 5xx. The
// smallest complete Go-side handoff is a direct chat task, which lets the
// existing task worker execute the mention. The source-aware send path keeps
// the durable channel-message evidence in the task's existing attribution
// contract; the channel bridge itself remains best-effort.
func (h *Handler) dispatchWorkspaceChannelMentions(ctx context.Context, channel db.WorkspaceChannel, message db.WorkspaceChannelMessage, actorType, actorID string) {
	if h == nil || actorType != "member" || h.TaskService == nil || h.TaskService.Queries == nil || h.TaskService.Bus == nil {
		return
	}
	actorUUID, err := util.ParseUUID(actorID)
	if err != nil {
		return
	}
	workspaceID := uuidToString(channel.WorkspaceID)
	for _, mention := range util.ParseMentions(message.Content) {
		if mention.Type != "agent" {
			continue
		}
		agentID, err := util.ParseUUID(mention.ID)
		if err != nil {
			continue
		}
		agent, err := h.TaskService.Queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{
			ID:          agentID,
			WorkspaceID: channel.WorkspaceID,
		})
		if err != nil || agent.ArchivedAt.Valid || !agent.RuntimeID.Valid {
			continue
		}
		if !h.canInvokeAgent(ctx, agent, "member", actorID, actorID, workspaceID) {
			continue
		}

		session, err := h.TaskService.Queries.CreateChatSession(ctx, db.CreateChatSessionParams{
			ID:          dbid.NewV7(),
			WorkspaceID: channel.WorkspaceID,
			AgentID:     agent.ID,
			CreatorID:   actorUUID,
			Title:       "Channel mention: " + channel.Name,
		})
		if err != nil {
			slog.WarnContext(ctx, "workspace channel mention session creation failed", "agent_id", mention.ID, "channel_id", uuidToString(channel.ID), "error", err)
			continue
		}
		prompt := fmt.Sprintf("You were mentioned in workspace channel #%s. Reply concisely and usefully to this channel message:\n\n%s", channel.Slug, message.Content)
		source := service.WorkspaceChannelMessageSource{
			WorkspaceID: channel.WorkspaceID,
			ChannelID:   channel.ID,
			MessageID:   message.ID,
			ActorType:   actorType,
			ActorID:     actorUUID,
		}
		if _, err := h.TaskService.SendDirectChatMessageFromWorkspaceChannel(ctx, session, agent, source, prompt); err != nil {
			slog.WarnContext(ctx, "workspace channel mention task creation failed", "agent_id", mention.ID, "channel_id", uuidToString(channel.ID), "error", err)
		}
	}
}

// workspaceChannelScope applies the same membership boundary whether the
// handler is reached through router middleware or called directly in a test.
// The latter matters because these handlers otherwise accept a forged
// X-Workspace-ID whenever the caller bypasses the router.
func (h *Handler) workspaceChannelScope(w http.ResponseWriter, r *http.Request) (string, pgtype.UUID, db.Member, bool) {
	if !h.workspaceChannelDependencies(w) {
		return "", pgtype.UUID{}, db.Member{}, false
	}
	workspaceID := h.resolveWorkspaceID(r)
	if workspaceID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return "", pgtype.UUID{}, db.Member{}, false
	}
	workspaceUUID, err := util.ParseUUID(workspaceID)
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid workspace_id")
		return "", pgtype.UUID{}, db.Member{}, false
	}
	member, ok := h.workspaceMember(w, r, workspaceID)
	if !ok {
		return "", pgtype.UUID{}, db.Member{}, false
	}
	// The middleware context is a fast path, not a replacement for the
	// workspace fence. Keep direct/context-injected calls fail-closed when a
	// member row was accidentally paired with a different workspace.
	if !workspaceChannelMemberMatchesWorkspace(member, workspaceID) {
		writeError(w, http.StatusNotFound, "workspace not found")
		return "", pgtype.UUID{}, db.Member{}, false
	}
	return workspaceID, workspaceUUID, member, true
}

func (h *Handler) workspaceChannelByID(ctx context.Context, channelID, workspaceID pgtype.UUID) (db.WorkspaceChannel, error) {
	if h == nil || h.DB == nil {
		return db.WorkspaceChannel{}, errors.New("database unavailable")
	}
	return scanWorkspaceChannel(h.DB.QueryRow(ctx, getWorkspaceChannelStatement, channelID, workspaceID))
}

func (h *Handler) ListWorkspaceChannels(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, _, ok := h.workspaceChannelScope(w, r)
	if !ok {
		return
	}
	rows, err := h.DB.Query(r.Context(), listWorkspaceChannelsStatement, workspaceUUID)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	channels, err := scanWorkspaceChannels(rows)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"channels": channels})
}

func (h *Handler) CreateWorkspaceChannel(w http.ResponseWriter, r *http.Request) {
	workspaceID, workspaceUUID, member, ok := h.workspaceChannelScope(w, r)
	if !ok {
		return
	}
	var body struct {
		Slug        string `json:"slug"`
		Name        string `json:"name"`
		Description string `json:"description"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	name := cleanWorkspaceChannelText(body.Name)
	if name == "" {
		// Keep the currently shipped request shape compatible: the frontend
		// normally sends both fields, but slug-only callers still receive a
		// usable display name while name-based callers get canonical slugging.
		name = cleanWorkspaceChannelText(body.Slug)
	}
	if name == "" {
		writeError(w, http.StatusBadRequest, "name is required")
		return
	}
	if !workspaceChannelTextWithinLimit(name, workspaceChannelNameMaxRunes) {
		writeError(w, http.StatusBadRequest, "name is too long")
		return
	}
	description := cleanWorkspaceChannelText(body.Description)
	if !workspaceChannelTextWithinLimit(description, workspaceChannelDescriptionMaxRunes) {
		writeError(w, http.StatusBadRequest, "description is too long")
		return
	}
	slug := cleanWorkspaceChannelText(body.Slug)
	if slug == "" {
		slug = slugifyWorkspaceChannelName(name)
	} else {
		var valid bool
		slug, valid = normalizeWorkspaceChannelSlug(slug)
		if !valid {
			writeError(w, http.StatusBadRequest, "invalid slug")
			return
		}
	}
	if slug == "" {
		writeError(w, http.StatusBadRequest, "name must contain a letter or number")
		return
	}
	actorUserID := requestUserID(r)
	if actorUserID == "" && member.UserID.Valid {
		actorUserID = uuidToString(member.UserID)
	}
	actorType, actorID := h.resolveActor(r, actorUserID, workspaceID)
	if actorType != "member" || actorID == "" {
		writeError(w, http.StatusForbidden, "agents cannot create workspace channels")
		return
	}
	createdBy, ok := parseUUIDOrBadRequest(w, actorID, "user id")
	if !ok {
		return
	}
	channel, err := scanWorkspaceChannel(h.DB.QueryRow(
		r.Context(),
		createWorkspaceChannelStatement,
		workspaceUUID,
		slug,
		name,
		description,
		createdBy,
	))
	if err != nil {
		if isUniqueViolation(err) || isCheckViolation(err) {
			writeErrorCode(w, http.StatusConflict, "channel_conflict", err.Error())
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	h.publishWorkspaceChannelEvent(workspaceChannelCreatedEvent, workspaceID, actorType, actorID, map[string]any{
		"channel": channel,
	})
	writeJSON(w, http.StatusCreated, channel)
}

func (h *Handler) GetWorkspaceChannel(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, _, ok := h.workspaceChannelScope(w, r)
	if !ok {
		return
	}
	channelID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "channel id")
	if !ok {
		return
	}
	channel, err := h.workspaceChannelByID(r.Context(), channelID, workspaceUUID)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	writeJSON(w, http.StatusOK, channel)
}

func (h *Handler) ListWorkspaceChannelMessages(w http.ResponseWriter, r *http.Request) {
	_, workspaceUUID, _, ok := h.workspaceChannelScope(w, r)
	if !ok {
		return
	}
	channelID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "channel id")
	if !ok {
		return
	}
	if _, err := h.workspaceChannelByID(r.Context(), channelID, workspaceUUID); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	limit, beforeCreatedAt, beforeID, err := parseChatMessagesPageParams(r)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	rows, err := h.DB.Query(r.Context(), listWorkspaceChannelMessagesStatement,
		workspaceUUID,
		channelID,
		beforeCreatedAt,
		beforeID,
		limit+1,
	)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	messages, err := scanWorkspaceChannelMessages(rows)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	hasMore := len(messages) > limit
	if hasMore {
		messages = messages[:limit]
	}
	var nextCursor *ChatMessagesCursorResponse
	if hasMore && len(messages) > 0 {
		last := messages[len(messages)-1]
		if createdAt := timestampToNanoPtr(last.CreatedAt); createdAt != nil {
			nextCursor = &ChatMessagesCursorResponse{
				CreatedAt: *createdAt,
				ID:        uuidToString(last.ID),
			}
		}
	}
	// The query is newest-first so the cursor always advances toward older
	// rows. Keep the response chronological to preserve the existing channel UI
	// contract, which appends messages in the order received.
	for left, right := 0, len(messages)-1; left < right; left, right = left+1, right-1 {
		messages[left], messages[right] = messages[right], messages[left]
	}
	writeJSON(w, http.StatusOK, workspaceChannelMessagesResponse{
		Messages:   messages,
		Limit:      limit,
		HasMore:    hasMore,
		NextCursor: nextCursor,
	})
}

func (h *Handler) CreateWorkspaceChannelMessage(w http.ResponseWriter, r *http.Request) {
	workspaceID, workspaceUUID, member, ok := h.workspaceChannelScope(w, r)
	if !ok {
		return
	}
	channelID, ok := parseUUIDOrBadRequest(w, chi.URLParam(r, "id"), "channel id")
	if !ok {
		return
	}
	channel, err := h.workspaceChannelByID(r.Context(), channelID, workspaceUUID)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			writeErrorCode(w, http.StatusNotFound, "not_found", "not found")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	var body struct {
		// These fields remain accepted for the current frontend contract, but
		// are intentionally not trusted. The authenticated request is the only
		// source of the persisted actor.
		AuthorType      string  `json:"author_type"`
		AuthorID        string  `json:"author_id"`
		Content         string  `json:"content"`
		ParentID        *string `json:"parent_id"`
		QuotedMessageID *string `json:"quoted_message_id"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeError(w, http.StatusBadRequest, "invalid json")
		return
	}
	content := cleanWorkspaceChannelText(body.Content)
	if content == "" {
		writeError(w, http.StatusBadRequest, "content is required")
		return
	}
	if !workspaceChannelTextWithinLimit(content, workspaceChannelMessageMaxRunes) {
		writeError(w, http.StatusBadRequest, "content is too long")
		return
	}
	actorUserID := requestUserID(r)
	if actorUserID == "" && member.UserID.Valid {
		actorUserID = uuidToString(member.UserID)
	}
	authorType, actorID := h.resolveActor(r, actorUserID, workspaceID)
	if authorType != "member" && authorType != "agent" {
		writeErrorCode(w, http.StatusForbidden, "invalid_actor", "authenticated actor is not valid")
		return
	}
	authorID, ok := parseUUIDOrBadRequest(w, actorID, "actor id")
	if !ok {
		return
	}
	var parentID, quotedMessageID pgtype.UUID
	if body.ParentID != nil && strings.TrimSpace(*body.ParentID) != "" {
		parentID, ok = parseUUIDOrBadRequest(w, strings.TrimSpace(*body.ParentID), "parent_id")
		if !ok {
			return
		}
		parent, err := h.workspaceChannelMessageByID(r.Context(), parentID, workspaceUUID)
		if err != nil {
			if errors.Is(err, pgx.ErrNoRows) {
				writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_message", "parent message not found")
				return
			}
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
		if parent.ChannelID != channelID {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_message", "parent message belongs to another channel")
			return
		}
	}
	if body.QuotedMessageID != nil && strings.TrimSpace(*body.QuotedMessageID) != "" {
		quotedMessageID, ok = parseUUIDOrBadRequest(w, strings.TrimSpace(*body.QuotedMessageID), "quoted_message_id")
		if !ok {
			return
		}
		quoted, err := h.workspaceChannelMessageByID(r.Context(), quotedMessageID, workspaceUUID)
		if err != nil {
			if errors.Is(err, pgx.ErrNoRows) {
				writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_message", "quoted message not found")
				return
			}
			writeError(w, http.StatusInternalServerError, "database error")
			return
		}
		if quoted.ChannelID != channelID {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_message", "quoted message belongs to another channel")
			return
		}
	}
	message, err := scanWorkspaceChannelMessage(h.DB.QueryRow(
		r.Context(),
		createWorkspaceChannelMessageStatement,
		workspaceUUID,
		channelID,
		authorType,
		authorID,
		content,
		parentID,
		quotedMessageID,
	))
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) || isCheckViolation(err) {
			writeErrorCode(w, http.StatusUnprocessableEntity, "invalid_message", "channel message is not valid")
			return
		}
		writeError(w, http.StatusInternalServerError, "database error")
		return
	}
	h.publishWorkspaceChannelEvent(workspaceChannelMessageEvent, workspaceID, authorType, actorID, map[string]any{
		"channel_id": uuidToString(channel.ID),
		"message":    message,
	})
	h.dispatchWorkspaceChannelMentions(r.Context(), channel, message, authorType, actorID)
	writeJSON(w, http.StatusCreated, message)
}
