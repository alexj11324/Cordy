package handler

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func newGuestTokenForTest(t *testing.T) string {
	t.Helper()
	buf := make([]byte, guestTokenHexLength/2)
	if _, err := rand.Read(buf); err != nil {
		t.Fatalf("generate guest token: %v", err)
	}
	return guestTokenPrefix + hex.EncodeToString(buf)
}

func guestSessionFixture(t *testing.T, status string) (userID, sessionID, token string) {
	t.Helper()
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	ctx := context.Background()
	userID = uuid.NewString()
	email := "guest-session-" + strings.ToLower(strings.ReplaceAll(t.Name(), "/", "-")) + "-" + userID + "@guest.patchbay.invalid"
	if _, err := testPool.Exec(ctx, `
		INSERT INTO "user" (id, name, email, is_guest)
		VALUES ($1, 'Guest', $2, TRUE)
	`, userID, email); err != nil {
		t.Fatalf("create guest user: %v", err)
	}

	sessionID = uuid.NewString()
	token = newGuestTokenForTest(t)
	if _, err := testPool.Exec(ctx, `
		INSERT INTO guest_session (id, user_id, token_hash, status)
		VALUES ($1, $2, $3, $4)
	`, sessionID, userID, hashGuestToken(token), status); err != nil {
		t.Fatalf("create guest session: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM guest_session WHERE user_id = $1`, userID)
		testPool.Exec(context.Background(), `DELETE FROM "user" WHERE id = $1`, userID)
	})
	return userID, sessionID, token
}

func guestRequest(t *testing.T, method, path string, body any, userID string) *http.Request {
	t.Helper()
	var payload bytes.Buffer
	if body != nil {
		if err := json.NewEncoder(&payload).Encode(body); err != nil {
			t.Fatalf("encode guest request: %v", err)
		}
	}
	req := httptest.NewRequest(method, path, &payload)
	req.Header.Set("Content-Type", "application/json")
	if userID != "" {
		req.Header.Set("X-User-ID", userID)
	}
	return req
}

func TestHashGuestTokenKnownVector(t *testing.T) {
	if got, want := hashGuestToken("pbg_abc"), "d4125582738e45be0ac496bcf3b314623489a65fedad259ecdcccd500e3b4990"; got != want {
		t.Fatalf("hashGuestToken(%q) = %q, want %q", "pbg_abc", got, want)
	}
}

func TestValidGuestTokenRequiresOpaquePBGShape(t *testing.T) {
	valid := guestTokenPrefix + strings.Repeat("a", guestTokenHexLength)
	for name, token := range map[string]string{
		"valid":        valid,
		"wrong prefix": "jwt_" + strings.Repeat("a", guestTokenHexLength),
		"short":        guestTokenPrefix + "a",
		"non hex":      guestTokenPrefix + strings.Repeat("g", guestTokenHexLength),
	} {
		t.Run(name, func(t *testing.T) {
			want := name == "valid"
			if got := validGuestToken(token); got != want {
				t.Fatalf("validGuestToken(%q) = %v, want %v", token, got, want)
			}
		})
	}
}

func TestGeneratedGuestTokenUsesOpaqueShape(t *testing.T) {
	token, err := generateGuestToken()
	if err != nil {
		t.Fatalf("generateGuestToken: %v", err)
	}
	if !validGuestToken(token) {
		t.Fatalf("generateGuestToken returned invalid token %q", token)
	}
}

func TestGuestTokenMatchesOnlyThePresentedOpaqueToken(t *testing.T) {
	token := guestTokenPrefix + strings.Repeat("a", guestTokenHexLength)
	sess := db.GuestSession{
		ID:        pgtype.UUID{Bytes: [16]byte{1}, Valid: true},
		UserID:    pgtype.UUID{Bytes: [16]byte{2}, Valid: true},
		TokenHash: hashGuestToken(token),
		Status:    guestSessionActive,
	}

	if !guestTokenMatches(sess, token) {
		t.Fatal("guestTokenMatches rejected the token used to create the hash")
	}
	if guestTokenMatches(sess, guestTokenPrefix+strings.Repeat("b", guestTokenHexLength)) {
		t.Fatal("guestTokenMatches accepted a different token")
	}
}

func TestGuestSessionIsActiveRequiresBoundActiveRow(t *testing.T) {
	base := db.GuestSession{
		ID:     pgtype.UUID{Bytes: [16]byte{1}, Valid: true},
		UserID: pgtype.UUID{Bytes: [16]byte{2}, Valid: true},
		Status: guestSessionActive,
	}
	if !guestSessionIsActive(base) {
		t.Fatal("active, bound guest session was rejected")
	}

	for name, sess := range map[string]db.GuestSession{
		"claimed":       {ID: base.ID, UserID: base.UserID, Status: guestSessionClaimed},
		"revoked":       {ID: base.ID, UserID: base.UserID, Status: guestSessionRevoked},
		"missing id":    {UserID: base.UserID, Status: guestSessionActive},
		"missing owner": {ID: base.ID, Status: guestSessionActive},
	} {
		t.Run(name, func(t *testing.T) {
			if guestSessionIsActive(sess) {
				t.Fatalf("guestSessionIsActive accepted %s session", name)
			}
		})
	}
}

func TestGuestSessionWireDoesNotExposeTokenHash(t *testing.T) {
	value, err := json.Marshal(guestSessionWire(db.GuestSession{
		ID:        pgtype.UUID{Bytes: [16]byte{1}, Valid: true},
		UserID:    pgtype.UUID{Bytes: [16]byte{2}, Valid: true},
		TokenHash: "secret-hash",
		Status:    guestSessionActive,
	}))
	if err != nil {
		t.Fatalf("marshal guest session: %v", err)
	}
	if strings.Contains(string(value), "token_hash") || strings.Contains(string(value), "secret-hash") {
		t.Fatalf("guest session response leaked token hash: %s", value)
	}
}

func TestGuestSessionHandlersRejectMachineCredentials(t *testing.T) {
	h := &Handler{}
	userID := "11111111-1111-1111-1111-111111111111"
	sessionID := "22222222-2222-2222-2222-222222222222"
	token := guestTokenPrefix + strings.Repeat("a", guestTokenHexLength)

	cases := []struct {
		name string
		call http.HandlerFunc
		body any
		path string
	}{
		{
			name: "create",
			call: h.CreateGuestSession,
			body: map[string]any{"user_id": userID, "token": token},
			path: "/api/guest-sessions",
		},
		{
			name: "get",
			call: h.GetGuestSession,
			path: "/api/guest-sessions/" + sessionID,
		},
		{
			name: "claim",
			call: h.ClaimGuestSession,
			body: map[string]any{"token": token},
			path: "/api/guest-sessions/" + sessionID + "/claim",
		},
		{
			name: "revoke",
			call: h.RevokeGuestSession,
			body: map[string]any{"token": token},
			path: "/api/guest-sessions/" + sessionID + "/revoke",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			req := guestRequest(t, http.MethodPost, tc.path, tc.body, userID)
			if tc.name == "get" {
				req.Method = http.MethodGet
			}
			req.Header.Set("X-Actor-Source", "task_token")
			if tc.name == "get" {
				req = withURLParam(req, "id", sessionID)
			} else if tc.name != "create" {
				req = withURLParam(req, "id", sessionID)
			}

			w := httptest.NewRecorder()
			tc.call(w, req)
			if w.Code != http.StatusForbidden {
				t.Fatalf("expected 403, got %d: %s", w.Code, w.Body.String())
			}
		})
	}
}

func TestGuestSessionHandlersFailClosedWhenQueriesAreUnavailable(t *testing.T) {
	h := &Handler{}
	userID := "11111111-1111-1111-1111-111111111111"
	sessionID := "22222222-2222-2222-2222-222222222222"
	token := guestTokenPrefix + strings.Repeat("a", guestTokenHexLength)

	cases := []struct {
		name   string
		call   http.HandlerFunc
		method string
		path   string
		body   any
	}{
		{name: "create", call: h.CreateGuestSession, method: http.MethodPost, path: "/api/guest-sessions", body: map[string]any{"token": token}},
		{name: "get", call: h.GetGuestSession, method: http.MethodGet, path: "/api/guest-sessions/" + sessionID},
		{name: "claim", call: h.ClaimGuestSession, method: http.MethodPost, path: "/api/guest-sessions/" + sessionID + "/claim", body: map[string]any{"token": token}},
		{name: "revoke", call: h.RevokeGuestSession, method: http.MethodPost, path: "/api/guest-sessions/" + sessionID + "/revoke", body: map[string]any{"token": token}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			req := guestRequest(t, tc.method, tc.path, tc.body, userID)
			req = withURLParam(req, "id", sessionID)
			w := httptest.NewRecorder()
			tc.call(w, req)
			if w.Code != http.StatusServiceUnavailable {
				t.Fatalf("expected 503, got %d: %s", w.Code, w.Body.String())
			}
		})
	}
}

func TestCreateGuestAuthFailsClosedWithoutTransactionDependencies(t *testing.T) {
	w := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPost, "/auth/guest", nil)
	(&Handler{}).CreateGuestAuth(w, req)
	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("expected 503, got %d: %s", w.Code, w.Body.String())
	}
}

func TestDecodeGuestJSONRejectsUnknownAndTrailingPayloads(t *testing.T) {
	cases := []string{
		`{"token":"pbg_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","unexpected":true}`,
		`{"token":"pbg_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}{"token":"pbg_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}`,
	}
	for _, payload := range cases {
		t.Run(payload[:min(len(payload), 20)], func(t *testing.T) {
			req := httptest.NewRequest(http.MethodPost, "/api/guest-sessions", strings.NewReader(payload))
			w := httptest.NewRecorder()
			var body struct {
				Token string `json:"token"`
			}
			if decodeGuestJSON(w, req, &body) {
				t.Fatal("decodeGuestJSON accepted an invalid payload")
			}
			if w.Code != http.StatusBadRequest {
				t.Fatalf("expected 400, got %d: %s", w.Code, w.Body.String())
			}
		})
	}
}

func TestGetGuestSessionIsOwnerScopedAndSanitized(t *testing.T) {
	ownerID, sessionID, _ := guestSessionFixture(t, guestSessionActive)
	foreignID := uuid.NewString()
	if _, err := testPool.Exec(context.Background(), `
		INSERT INTO "user" (id, name, email, is_guest)
		VALUES ($1, 'Foreign', $2, FALSE)
	`, foreignID, "foreign-"+foreignID+"@example.com"); err != nil {
		t.Fatalf("create foreign user: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM "user" WHERE id = $1`, foreignID)
	})

	path := "/api/guest-sessions/" + sessionID
	w := httptest.NewRecorder()
	req := withURLParam(guestRequest(t, http.MethodGet, path, nil, ownerID), "id", sessionID)
	testHandler.GetGuestSession(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("owner get: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	if strings.Contains(w.Body.String(), "token_hash") {
		t.Fatalf("owner get leaked token_hash: %s", w.Body.String())
	}

	w = httptest.NewRecorder()
	req = withURLParam(guestRequest(t, http.MethodGet, path, nil, foreignID), "id", sessionID)
	testHandler.GetGuestSession(w, req)
	if w.Code != http.StatusNotFound {
		t.Fatalf("foreign get: expected 404, got %d: %s", w.Code, w.Body.String())
	}
}

func TestRevokeGuestSessionRequiresGuestOwnerAndPresentedToken(t *testing.T) {
	ownerID, sessionID, token := guestSessionFixture(t, guestSessionActive)
	foreignID := uuid.NewString()
	if _, err := testPool.Exec(context.Background(), `
		INSERT INTO "user" (id, name, email, is_guest)
		VALUES ($1, 'Foreign formal user', $2, FALSE)
	`, foreignID, "foreign-revoke-"+foreignID+"@example.com"); err != nil {
		t.Fatalf("create foreign user: %v", err)
	}
	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM "user" WHERE id = $1`, foreignID)
	})

	path := "/api/guest-sessions/" + sessionID + "/revoke"
	w := httptest.NewRecorder()
	req := withURLParam(guestRequest(t, http.MethodPost, path, map[string]any{
		"token": token,
	}, foreignID), "id", sessionID)
	testHandler.RevokeGuestSession(w, req)
	if w.Code != http.StatusNotFound {
		t.Fatalf("foreign owner: expected 404, got %d: %s", w.Code, w.Body.String())
	}

	w = httptest.NewRecorder()
	req = withURLParam(guestRequest(t, http.MethodPost, path, map[string]any{
		"token": guestTokenPrefix + strings.Repeat("b", guestTokenHexLength),
	}, ownerID), "id", sessionID)
	testHandler.RevokeGuestSession(w, req)
	if w.Code != http.StatusNotFound {
		t.Fatalf("wrong token: expected 404, got %d: %s", w.Code, w.Body.String())
	}
}

func TestClaimGuestSessionRequiresTokenAndFormalOwner(t *testing.T) {
	guestID, sessionID, token := guestSessionFixture(t, guestSessionActive)
	path := "/api/guest-sessions/" + sessionID

	w := httptest.NewRecorder()
	req := withURLParam(guestRequest(t, http.MethodPost, path+"/claim", map[string]any{
		"token": token,
	}, guestID), "id", sessionID)
	testHandler.ClaimGuestSession(w, req)
	if w.Code != http.StatusForbidden {
		t.Fatalf("guest claimant: expected 403, got %d: %s", w.Code, w.Body.String())
	}

	w = httptest.NewRecorder()
	req = withURLParam(guestRequest(t, http.MethodPost, path+"/claim", map[string]any{}, testUserID), "id", sessionID)
	testHandler.ClaimGuestSession(w, req)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("missing token: expected 400, got %d: %s", w.Code, w.Body.String())
	}

	w = httptest.NewRecorder()
	req = withURLParam(guestRequest(t, http.MethodPost, path+"/claim", map[string]any{
		"token": token,
	}, testUserID), "id", sessionID)
	testHandler.ClaimGuestSession(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("formal claimant: expected 200, got %d: %s", w.Code, w.Body.String())
	}
	if strings.Contains(w.Body.String(), "token_hash") {
		t.Fatalf("claim leaked token_hash: %s", w.Body.String())
	}

	w = httptest.NewRecorder()
	req = withURLParam(guestRequest(t, http.MethodPost, path+"/claim", map[string]any{
		"token": token,
	}, testUserID), "id", sessionID)
	testHandler.ClaimGuestSession(w, req)
	if w.Code != http.StatusConflict {
		t.Fatalf("second claim: expected 409, got %d: %s", w.Code, w.Body.String())
	}
}

func TestClaimGuestSessionRejectsRevokedSession(t *testing.T) {
	_, sessionID, token := guestSessionFixture(t, guestSessionRevoked)
	path := "/api/guest-sessions/" + sessionID + "/claim"
	w := httptest.NewRecorder()
	req := withURLParam(guestRequest(t, http.MethodPost, path, map[string]any{
		"token": token,
	}, testUserID), "id", sessionID)
	testHandler.ClaimGuestSession(w, req)
	if w.Code != http.StatusConflict {
		t.Fatalf("revoked claim: expected 409, got %d: %s", w.Code, w.Body.String())
	}
}

func TestRevokeGuestSessionRequiresTokenAndIsIdempotentlyBound(t *testing.T) {
	ownerID, sessionID, token := guestSessionFixture(t, guestSessionActive)
	path := "/api/guest-sessions/" + sessionID

	w := httptest.NewRecorder()
	req := withURLParam(guestRequest(t, http.MethodPost, path+"/revoke", map[string]any{
		"token": token,
	}, ownerID), "id", sessionID)
	testHandler.RevokeGuestSession(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("revoke: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	w = httptest.NewRecorder()
	req = withURLParam(guestRequest(t, http.MethodPost, path+"/revoke", map[string]any{
		"token": token,
	}, ownerID), "id", sessionID)
	testHandler.RevokeGuestSession(w, req)
	if w.Code != http.StatusConflict {
		t.Fatalf("repeat revoke: expected 409, got %d: %s", w.Code, w.Body.String())
	}
}

func TestCreateGuestAuthMarksUserAsGuest(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("database not available")
	}

	w := httptest.NewRecorder()
	req := guestRequest(t, http.MethodPost, "/auth/guest", nil, "")
	testHandler.CreateGuestAuth(w, req)
	if w.Code != http.StatusOK {
		t.Fatalf("create guest: expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var body struct {
		Token string `json:"token"`
		User  struct {
			ID      string `json:"id"`
			IsGuest *bool  `json:"is_guest"`
		} `json:"user"`
	}
	bodyBytes := w.Body.Bytes()
	if err := json.Unmarshal(bodyBytes, &body); err != nil {
		t.Fatalf("decode guest response: %v (body: %s)", err, bodyBytes)
	}
	if body.Token == "" {
		t.Fatalf("create guest: empty token (body: %s)", bodyBytes)
	}
	if body.User.IsGuest == nil || *body.User.IsGuest != true {
		t.Fatalf("create guest: expected user.is_guest=true, got %s", bodyBytes)
	}

	t.Cleanup(func() {
		testPool.Exec(context.Background(), `DELETE FROM guest_session WHERE user_id = $1`, body.User.ID)
		testPool.Exec(context.Background(), `DELETE FROM "user" WHERE id = $1`, body.User.ID)
	})
}
