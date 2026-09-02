package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/google/uuid"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestSlugifyWorkspaceChannelName(t *testing.T) {
	tests := []struct {
		name  string
		input string
		want  string
	}{
		{name: "spaces and punctuation", input: "  Project Alpha / v2! ", want: "project-alpha-v2"},
		{name: "unicode letters and numbers", input: "研发 频道 2", want: "研发-频道-2"},
		{name: "no slug characters", input: "---...", want: ""},
		{name: "bounded", input: strings.Repeat("a", workspaceChannelSlugMaxRunes+10), want: strings.Repeat("a", workspaceChannelSlugMaxRunes)},
		{name: "bounded without trailing separator", input: strings.Repeat("a", workspaceChannelSlugMaxRunes-1) + " b", want: strings.Repeat("a", workspaceChannelSlugMaxRunes-1)},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := slugifyWorkspaceChannelName(tt.input); got != tt.want {
				t.Fatalf("slugifyWorkspaceChannelName(%q) = %q, want %q", tt.input, got, tt.want)
			}
		})
	}
}

func TestNormalizeWorkspaceChannelSlug(t *testing.T) {
	tests := []struct {
		name  string
		input string
		want  string
		valid bool
	}{
		{name: "trim and lowercase", input: "  Project-2  ", want: "project-2", valid: true},
		{name: "unicode", input: "研发-频道", want: "研发-频道", valid: true},
		{name: "leading hyphen", input: "-project", valid: false},
		{name: "trailing hyphen", input: "project-", valid: false},
		{name: "duplicate hyphen", input: "project--two", valid: false},
		{name: "punctuation", input: "project_two", valid: false},
		{name: "too long", input: strings.Repeat("a", workspaceChannelSlugMaxRunes+1), valid: false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, valid := normalizeWorkspaceChannelSlug(tt.input)
			if valid != tt.valid || got != tt.want {
				t.Fatalf("normalizeWorkspaceChannelSlug(%q) = (%q, %t), want (%q, %t)", tt.input, got, valid, tt.want, tt.valid)
			}
		})
	}
}

func TestWorkspaceChannelMemberWorkspaceFence(t *testing.T) {
	workspaceID := uuid.NewString()
	foreignWorkspaceID := uuid.NewString()
	tests := []struct {
		name   string
		member db.Member
		valid  bool
	}{
		{
			name:   "matching workspace",
			member: db.Member{WorkspaceID: parseUUID(workspaceID)},
			valid:  true,
		},
		{
			name:   "foreign workspace",
			member: db.Member{WorkspaceID: parseUUID(foreignWorkspaceID)},
			valid:  false,
		},
		{
			name:  "missing workspace",
			valid: false,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := workspaceChannelMemberMatchesWorkspace(tt.member, workspaceID); got != tt.valid {
				t.Fatalf("workspaceChannelMemberMatchesWorkspace() = %t, want %t", got, tt.valid)
			}
		})
	}
}

func TestWorkspaceChannelCursorParsingBoundaries(t *testing.T) {
	beforeID := uuid.NewString()
	beforeAt := "2026-09-02T07:13:16.123456789Z"
	tests := []struct {
		name      string
		query     string
		wantLimit int
		wantValid bool
	}{
		{name: "default", query: "", wantLimit: 50, wantValid: false},
		{name: "minimum", query: "?limit=1", wantLimit: 1, wantValid: false},
		{name: "maximum", query: "?limit=100", wantLimit: 100, wantValid: false},
		{name: "full cursor", query: "?limit=2&before_created_at=" + url.QueryEscape(beforeAt) + "&before_id=" + beforeID, wantLimit: 2, wantValid: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			limit, beforeTime, gotBeforeID, err := parseChatMessagesPageParams(httptest.NewRequest(http.MethodGet, "/api/workspace-channels/channel/messages"+tt.query, nil))
			if err != nil {
				t.Fatalf("parseChatMessagesPageParams() error = %v", err)
			}
			if limit != tt.wantLimit || beforeTime.Valid != tt.wantValid {
				t.Fatalf("parsed page = (limit %d, cursor valid %t), want (%d, %t)", limit, beforeTime.Valid, tt.wantLimit, tt.wantValid)
			}
			if tt.wantValid && uuidToString(gotBeforeID) != beforeID {
				t.Fatalf("before id = %s, want %s", uuidToString(gotBeforeID), beforeID)
			}
			if tt.wantValid && beforeTime.Time.Format("2006-01-02T15:04:05.999999999Z07:00") != beforeAt {
				t.Fatalf("before time lost precision: %s", beforeTime.Time.Format("2006-01-02T15:04:05.999999999Z07:00"))
			}
		})
	}

	for _, query := range []string{"?limit=0", "?limit=101", "?before_created_at=" + url.QueryEscape(beforeAt), "?before_id=" + beforeID} {
		t.Run("reject "+query, func(t *testing.T) {
			if _, _, _, err := parseChatMessagesPageParams(httptest.NewRequest(http.MethodGet, "/api/workspace-channels/channel/messages"+query, nil)); err == nil {
				t.Fatalf("parseChatMessagesPageParams(%q) accepted invalid query", query)
			}
		})
	}
}

func TestWorkspaceChannelSQLContractHasStableFences(t *testing.T) {
	checks := []struct {
		name string
		sql  string
		want []string
	}{
		{
			name: "channel list",
			sql:  listWorkspaceChannelsStatement,
			want: []string{"workspace_id = $1", "archived_at IS NULL", "ORDER BY created_at, id"},
		},
		{
			name: "message list cursor",
			sql:  listWorkspaceChannelMessagesStatement,
			want: []string{"message.workspace_id = $1", "message.channel_id = $2", "channel.archived_at IS NULL", "(message.created_at, message.id) <", "ORDER BY message.created_at DESC, message.id DESC", "LIMIT $5"},
		},
		{
			name: "message parent and quote",
			sql:  createWorkspaceChannelMessageStatement,
			want: []string{"WHERE id = $6 AND workspace_id = $1 AND channel_id = $2", "WHERE id = $7 AND workspace_id = $1 AND channel_id = $2", "AND archived_at IS NULL"},
		},
	}
	for _, check := range checks {
		t.Run(check.name, func(t *testing.T) {
			for _, fragment := range check.want {
				if !strings.Contains(check.sql, fragment) {
					t.Fatalf("SQL missing %q", fragment)
				}
			}
		})
	}
}

func TestWorkspaceChannelHandlersReturnUnavailableWithoutDependencies(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/api/workspace-channels", nil)
	resp := httptest.NewRecorder()
	var handler *Handler
	handler.ListWorkspaceChannels(resp, req)
	if resp.Code != http.StatusServiceUnavailable {
		t.Fatalf("nil handler list status = %d, want %d", resp.Code, http.StatusServiceUnavailable)
	}
}

func TestWorkspaceChannelEventPublisherIncludesActorAndPayload(t *testing.T) {
	bus := events.New()
	var got events.Event
	bus.Subscribe(workspaceChannelMessageEvent, func(event events.Event) { got = event })
	handler := &Handler{Bus: bus}
	message := db.WorkspaceChannelMessage{Content: "hello"}
	handler.publishWorkspaceChannelEvent(workspaceChannelMessageEvent, "workspace-id", "member", "actor-id", map[string]any{
		"channel_id": "channel-id",
		"message":    message,
	})
	if got.Type != workspaceChannelMessageEvent || got.WorkspaceID != "workspace-id" || got.ActorType != "member" || got.ActorID != "actor-id" {
		t.Fatalf("event routing fields = %#v", got)
	}
	payload, ok := got.Payload.(map[string]any)
	if !ok || payload["channel_id"] != "channel-id" {
		t.Fatalf("event payload routing fields = %#v", got.Payload)
	}
	if published, ok := payload["message"].(db.WorkspaceChannelMessage); !ok || published.Content != message.Content {
		t.Fatalf("event payload message = %#v", payload["message"])
	}
}

func TestWorkspaceChannelMentionBridgeIsBestEffortWithoutDependencies(t *testing.T) {
	channel := db.WorkspaceChannel{Slug: "general", Name: "General"}
	message := db.WorkspaceChannelMessage{Content: "hello [@agent](mention://agent/" + uuid.NewString() + ")"}
	var nilHandler *Handler
	nilHandler.dispatchWorkspaceChannelMentions(t.Context(), channel, message, "member", uuid.NewString())
	(&Handler{}).dispatchWorkspaceChannelMentions(t.Context(), channel, message, "member", uuid.NewString())
	(&Handler{}).dispatchWorkspaceChannelMentions(t.Context(), channel, message, "agent", uuid.NewString())
}

func createWorkspaceChannelForTest(t *testing.T, slug string) db.WorkspaceChannel {
	t.Helper()
	req := newRequest("POST", "/api/workspace-channels?workspace_id="+testWorkspaceID, map[string]any{
		"slug": slug,
		"name": "Channel " + slug,
	})
	resp := httptest.NewRecorder()
	testHandler.CreateWorkspaceChannel(resp, req)
	if resp.Code != 201 {
		t.Fatalf("create workspace channel status = %d, body = %s", resp.Code, resp.Body.String())
	}
	var channel db.WorkspaceChannel
	if err := json.NewDecoder(resp.Body).Decode(&channel); err != nil {
		t.Fatalf("decode workspace channel: %v", err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(t.Context(), `DELETE FROM workspace_channel_message WHERE channel_id = $1`, channel.ID)
		_, _ = testPool.Exec(t.Context(), `DELETE FROM workspace_channel WHERE id = $1`, channel.ID)
	})
	return channel
}

func TestWorkspaceChannelCreateUsesSchemaAndCreator(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}
	channel := createWorkspaceChannelForTest(t, "schema-"+uuid.NewString())
	if uuidToString(channel.CreatedBy) != testUserID {
		t.Fatalf("created_by = %s, want authenticated user %s", uuidToString(channel.CreatedBy), testUserID)
	}
	if channel.Name == "" || channel.Description != "" {
		t.Fatalf("channel fields = name %q description %q, want non-empty name and empty default description", channel.Name, channel.Description)
	}
}

func TestWorkspaceChannelMessageRejectsCrossChannelParent(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}
	parentChannel := createWorkspaceChannelForTest(t, "parent-"+uuid.NewString())
	childChannel := createWorkspaceChannelForTest(t, "child-"+uuid.NewString())

	parentReq := newRequest("POST", "/api/workspace-channels/"+uuidToString(parentChannel.ID)+"/messages", map[string]any{
		"author_type": "member",
		"author_id":   testUserID,
		"content":     "parent",
	})
	parentReq = withURLParam(parentReq, "id", uuidToString(parentChannel.ID))
	parentResp := httptest.NewRecorder()
	testHandler.CreateWorkspaceChannelMessage(parentResp, parentReq)
	if parentResp.Code != 201 {
		t.Fatalf("create parent message status = %d, body = %s", parentResp.Code, parentResp.Body.String())
	}
	var parent db.WorkspaceChannelMessage
	if err := json.NewDecoder(parentResp.Body).Decode(&parent); err != nil {
		t.Fatalf("decode parent message: %v", err)
	}

	childReq := newRequest("POST", "/api/workspace-channels/"+uuidToString(childChannel.ID)+"/messages", map[string]any{
		"author_type": "member",
		"author_id":   testUserID,
		"content":     "cross-channel child",
		"parent_id":   uuidToString(parent.ID),
	})
	childReq = withURLParam(childReq, "id", uuidToString(childChannel.ID))
	childResp := httptest.NewRecorder()
	testHandler.CreateWorkspaceChannelMessage(childResp, childReq)
	if childResp.Code != 422 {
		t.Fatalf("cross-channel parent status = %d, body = %s; want 422", childResp.Code, childResp.Body.String())
	}

	quoteReq := newRequest("POST", "/api/workspace-channels/"+uuidToString(childChannel.ID)+"/messages", map[string]any{
		"author_type":       "member",
		"author_id":         testUserID,
		"content":           "cross-channel quote",
		"quoted_message_id": uuidToString(parent.ID),
	})
	quoteReq = withURLParam(quoteReq, "id", uuidToString(childChannel.ID))
	quoteResp := httptest.NewRecorder()
	testHandler.CreateWorkspaceChannelMessage(quoteResp, quoteReq)
	if quoteResp.Code != 422 {
		t.Fatalf("cross-channel quote status = %d, body = %s; want 422", quoteResp.Code, quoteResp.Body.String())
	}
}

func TestWorkspaceChannelScopeRejectsInvalidOrForeignWorkspace(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}

	invalidReq := newRequest("GET", "/api/workspace-channels", nil)
	invalidReq.Header.Set("X-Workspace-ID", "not-a-uuid")
	invalidResp := httptest.NewRecorder()
	testHandler.ListWorkspaceChannels(invalidResp, invalidReq)
	if invalidResp.Code != 400 {
		t.Fatalf("invalid workspace status = %d, body = %s; want 400", invalidResp.Code, invalidResp.Body.String())
	}

	foreignReq := newRequest("GET", "/api/workspace-channels", nil)
	foreignReq.Header.Set("X-Workspace-ID", uuid.NewString())
	foreignResp := httptest.NewRecorder()
	testHandler.ListWorkspaceChannels(foreignResp, foreignReq)
	if foreignResp.Code != 404 {
		t.Fatalf("foreign workspace status = %d, body = %s; want 404", foreignResp.Code, foreignResp.Body.String())
	}
}

func TestWorkspaceChannelMessageUsesAuthenticatedActor(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}
	channel := createWorkspaceChannelForTest(t, "actor-"+uuid.NewString())
	req := newRequest("POST", "/api/workspace-channels/"+uuidToString(channel.ID)+"/messages", map[string]any{
		"author_type": "agent",
		"author_id":   uuid.NewString(),
		"content":     "the server derives this actor",
	})
	req = withURLParam(req, "id", uuidToString(channel.ID))
	resp := httptest.NewRecorder()
	testHandler.CreateWorkspaceChannelMessage(resp, req)
	if resp.Code != http.StatusCreated {
		t.Fatalf("spoofed author status = %d, body = %s", resp.Code, resp.Body.String())
	}
	var message db.WorkspaceChannelMessage
	if err := json.NewDecoder(resp.Body).Decode(&message); err != nil {
		t.Fatalf("decode message: %v", err)
	}
	if message.AuthorType != "member" || uuidToString(message.AuthorID) != testUserID {
		t.Fatalf("persisted actor = (%q, %q), want authenticated member (%q, %q)", message.AuthorType, uuidToString(message.AuthorID), "member", testUserID)
	}
}

func TestWorkspaceChannelArchiveIsHiddenFromReads(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}
	channel := createWorkspaceChannelForTest(t, "archive-"+uuid.NewString())
	if _, err := testPool.Exec(t.Context(), `UPDATE workspace_channel SET archived_at = now() WHERE id = $1`, channel.ID); err != nil {
		t.Fatalf("archive channel: %v", err)
	}

	listReq := newRequest("GET", "/api/workspace-channels", nil)
	listResp := httptest.NewRecorder()
	testHandler.ListWorkspaceChannels(listResp, listReq)
	if listResp.Code != http.StatusOK {
		t.Fatalf("list archived channel status = %d, body = %s", listResp.Code, listResp.Body.String())
	}
	var listed struct {
		Channels []db.WorkspaceChannel `json:"channels"`
	}
	if err := json.NewDecoder(listResp.Body).Decode(&listed); err != nil {
		t.Fatalf("decode channels: %v", err)
	}
	for _, listedChannel := range listed.Channels {
		if listedChannel.ID == channel.ID {
			t.Fatalf("archived channel %s was returned by list", uuidToString(channel.ID))
		}
	}

	getReq := newRequest("GET", "/api/workspace-channels/"+uuidToString(channel.ID), nil)
	getReq = withURLParam(getReq, "id", uuidToString(channel.ID))
	getResp := httptest.NewRecorder()
	testHandler.GetWorkspaceChannel(getResp, getReq)
	if getResp.Code != http.StatusNotFound {
		t.Fatalf("get archived channel status = %d, body = %s; want 404", getResp.Code, getResp.Body.String())
	}
}

func TestWorkspaceChannelMessagesUseCursorPagination(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database-backed handler test fixture is required")
	}
	channel := createWorkspaceChannelForTest(t, "page-"+uuid.NewString())
	createdIDs := make(map[string]struct{}, 3)
	for i := 0; i < 3; i++ {
		req := newRequest("POST", "/api/workspace-channels/"+uuidToString(channel.ID)+"/messages", map[string]any{
			"author_type": "member",
			"author_id":   testUserID,
			"content":     "page message " + uuid.NewString(),
		})
		req = withURLParam(req, "id", uuidToString(channel.ID))
		resp := httptest.NewRecorder()
		testHandler.CreateWorkspaceChannelMessage(resp, req)
		if resp.Code != http.StatusCreated {
			t.Fatalf("create page message status = %d, body = %s", resp.Code, resp.Body.String())
		}
		var message db.WorkspaceChannelMessage
		if err := json.NewDecoder(resp.Body).Decode(&message); err != nil {
			t.Fatalf("decode page message: %v", err)
		}
		createdIDs[uuidToString(message.ID)] = struct{}{}
	}

	firstReq := newRequest("GET", "/api/workspace-channels/"+uuidToString(channel.ID)+"/messages?limit=2", nil)
	firstReq = withURLParam(firstReq, "id", uuidToString(channel.ID))
	firstResp := httptest.NewRecorder()
	testHandler.ListWorkspaceChannelMessages(firstResp, firstReq)
	if firstResp.Code != http.StatusOK {
		t.Fatalf("first page status = %d, body = %s", firstResp.Code, firstResp.Body.String())
	}
	var firstPage workspaceChannelMessagesResponse
	if err := json.NewDecoder(firstResp.Body).Decode(&firstPage); err != nil {
		t.Fatalf("decode first page: %v", err)
	}
	if len(firstPage.Messages) != 2 || firstPage.Limit != 2 || !firstPage.HasMore || firstPage.NextCursor == nil {
		t.Fatalf("first page = %#v, want two rows with cursor", firstPage)
	}
	seenIDs := make(map[string]struct{}, len(firstPage.Messages))
	for _, message := range firstPage.Messages {
		seenIDs[uuidToString(message.ID)] = struct{}{}
	}

	secondPath := "/api/workspace-channels/" + uuidToString(channel.ID) + "/messages?limit=2&before_created_at=" + url.QueryEscape(firstPage.NextCursor.CreatedAt) + "&before_id=" + url.QueryEscape(firstPage.NextCursor.ID)
	secondReq := newRequest("GET", secondPath, nil)
	secondReq = withURLParam(secondReq, "id", uuidToString(channel.ID))
	secondResp := httptest.NewRecorder()
	testHandler.ListWorkspaceChannelMessages(secondResp, secondReq)
	if secondResp.Code != http.StatusOK {
		t.Fatalf("second page status = %d, body = %s", secondResp.Code, secondResp.Body.String())
	}
	var secondPage workspaceChannelMessagesResponse
	if err := json.NewDecoder(secondResp.Body).Decode(&secondPage); err != nil {
		t.Fatalf("decode second page: %v", err)
	}
	if len(secondPage.Messages) != 1 || secondPage.HasMore || secondPage.NextCursor != nil {
		t.Fatalf("second page = %#v, want final single row", secondPage)
	}
	for _, message := range secondPage.Messages {
		id := uuidToString(message.ID)
		if _, duplicate := seenIDs[id]; duplicate {
			t.Fatalf("cursor page repeated message %s", id)
		}
		seenIDs[id] = struct{}{}
	}
	if len(seenIDs) != len(createdIDs) {
		t.Fatalf("cursor pages covered %d/%d created messages", len(seenIDs), len(createdIDs))
	}
}
