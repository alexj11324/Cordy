package handler

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/orvilo-ai/orvilo/server/internal/auth"
)

func TestDeviceAuthorizationCodeHelpers(t *testing.T) {
	for i := 0; i < 100; i++ {
		deviceCode, err := generateDeviceCode()
		if err != nil {
			t.Fatalf("generate device code: %v", err)
		}
		if !strings.HasPrefix(deviceCode, deviceAuthorizationCodePrefix) {
			t.Fatalf("device code = %q, want %q prefix", deviceCode, deviceAuthorizationCodePrefix)
		}

		userCode, err := generateDeviceUserCode()
		if err != nil {
			t.Fatalf("generate user code: %v", err)
		}
		compact := normalizeDeviceUserCode(userCode)
		if !validDeviceUserCode(compact) || len(userCode) != deviceAuthorizationUserCodeLength+1 {
			t.Fatalf("user code = %q, compact = %q, want XXXX-XXXX and valid compact code", userCode, compact)
		}
	}

	for _, tc := range []struct {
		input string
		want  string
	}{
		{input: "abcd-efgh", want: "ABCDEFGH"},
		{input: " ABCD EFGH ", want: "ABCDEFGH"},
		{input: "ABCD\tEFGH", want: "ABCDEFGH"},
	} {
		if got := normalizeDeviceUserCode(tc.input); got != tc.want {
			t.Errorf("normalizeDeviceUserCode(%q) = %q, want %q", tc.input, got, tc.want)
		}
	}
}

func TestDeviceAuthorizationRejectsGuestActors(t *testing.T) {
	for _, name := range []string{"inspect", "decision"} {
		t.Run(name, func(t *testing.T) {
			req := newRequest("POST", "/api/auth/device/"+name, nil)
			req.Header.Set("X-Guest-User", "true")
			w := httptest.NewRecorder()
			if requireDeviceAuthorizationActor(w, req) {
				t.Fatal("guest actor passed device authorization gate")
			}
			if w.Code != http.StatusForbidden {
				t.Fatalf("status = %d, want %d", w.Code, http.StatusForbidden)
			}
		})
	}
}

func TestDeviceAuthorizationLifecycle(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("handler database is unavailable")
	}
	originalConfig := testHandler.cfg
	testHandler.cfg.AppURL = "https://app.example.test"
	t.Cleanup(func() { testHandler.cfg = originalConfig })

	createWriter := httptest.NewRecorder()
	testHandler.CreateDeviceAuthorization(createWriter, newRequest("POST", "/api/auth/device/code", map[string]any{
		"client_name": "orvilo-test-cli",
	}))
	if createWriter.Code != http.StatusOK {
		t.Fatalf("create status = %d: %s", createWriter.Code, createWriter.Body.String())
	}
	var created DeviceAuthorizationCodeResponse
	if err := json.NewDecoder(createWriter.Body).Decode(&created); err != nil {
		t.Fatalf("decode create response: %v", err)
	}
	if created.DeviceCode == "" || created.UserCode == "" || created.VerificationURI != "https://app.example.test/device" {
		t.Fatalf("create response = %#v", created)
	}

	row, err := testHandler.Queries.GetDeviceAuthorizationByDeviceCodeHash(context.Background(), auth.HashToken(created.DeviceCode))
	if err != nil {
		t.Fatalf("load device authorization: %v", err)
	}
	issuedToken := ""
	t.Cleanup(func() {
		if issuedToken != "" {
			_, _ = testPool.Exec(context.Background(), "DELETE FROM personal_access_token WHERE token_hash = $1", auth.HashToken(issuedToken))
		}
		_, _ = testPool.Exec(context.Background(), "DELETE FROM device_authorization WHERE id = $1", row.ID)
	})

	inspectWriter := httptest.NewRecorder()
	testHandler.InspectDeviceAuthorization(inspectWriter, newRequest("POST", "/api/auth/device/inspect", map[string]any{
		"user_code": created.UserCode,
	}))
	if inspectWriter.Code != http.StatusOK {
		t.Fatalf("inspect status = %d: %s", inspectWriter.Code, inspectWriter.Body.String())
	}
	var inspected DeviceAuthorizationInspectResponse
	if err := json.NewDecoder(inspectWriter.Body).Decode(&inspected); err != nil {
		t.Fatalf("decode inspect response: %v", err)
	}
	if inspected.ClientName != row.ClientName || inspected.ExpiresAt == "" {
		t.Fatalf("inspect response = %#v", inspected)
	}

	pendingWriter := httptest.NewRecorder()
	testHandler.ExchangeDeviceAuthorizationToken(pendingWriter, newRequest("POST", "/api/auth/device/token", map[string]any{
		"device_code": created.DeviceCode,
	}))
	if pendingWriter.Code != http.StatusBadRequest {
		t.Fatalf("pending status = %d: %s", pendingWriter.Code, pendingWriter.Body.String())
	}
	var pending deviceAuthorizationTokenError
	if err := json.NewDecoder(pendingWriter.Body).Decode(&pending); err != nil {
		t.Fatalf("decode pending response: %v", err)
	}
	if pending.Error != "authorization_pending" {
		t.Fatalf("pending error = %#v, want authorization_pending", pending)
	}

	approveWriter := httptest.NewRecorder()
	approve := true
	testHandler.DecideDeviceAuthorization(approveWriter, newRequest("POST", "/api/auth/device/decision", map[string]any{
		"user_code": created.UserCode,
		"approve":   approve,
	}))
	if approveWriter.Code != http.StatusNoContent {
		t.Fatalf("approve status = %d: %s", approveWriter.Code, approveWriter.Body.String())
	}

	tokenWriter := httptest.NewRecorder()
	testHandler.ExchangeDeviceAuthorizationToken(tokenWriter, newRequest("POST", "/api/auth/device/token", map[string]any{
		"device_code": created.DeviceCode,
	}))
	if tokenWriter.Code != http.StatusOK {
		t.Fatalf("exchange status = %d: %s", tokenWriter.Code, tokenWriter.Body.String())
	}
	var token DeviceAuthorizationTokenResponse
	if err := json.NewDecoder(tokenWriter.Body).Decode(&token); err != nil {
		t.Fatalf("decode token response: %v", err)
	}
	if token.TokenType != "Bearer" || !strings.HasPrefix(token.AccessToken, "ovy_") {
		t.Fatalf("token response = %#v", token)
	}
	issuedToken = token.AccessToken

	secondWriter := httptest.NewRecorder()
	testHandler.ExchangeDeviceAuthorizationToken(secondWriter, newRequest("POST", "/api/auth/device/token", map[string]any{
		"device_code": created.DeviceCode,
	}))
	if secondWriter.Code != http.StatusBadRequest || !strings.Contains(secondWriter.Body.String(), `"expired_token"`) {
		t.Fatalf("second exchange = %d: %s, want one-time expired_token", secondWriter.Code, secondWriter.Body.String())
	}

	pat, err := testHandler.Queries.GetPersonalAccessTokenByHash(context.Background(), auth.HashToken(token.AccessToken))
	if err != nil {
		t.Fatalf("issued PAT is not usable: %v", err)
	}
	if uuidToString(pat.UserID) != testUserID || pat.ExpiresAt.Time.Before(time.Now()) {
		t.Fatalf("issued PAT = %#v, want current test user and future expiry", pat)
	}
}

func TestExpiredPendingDeviceAuthorizationReturnsExpiredToken(t *testing.T) {
	if testHandler == nil || testPool == nil {
		t.Skip("handler database is unavailable")
	}
	originalConfig := testHandler.cfg
	testHandler.cfg.AppURL = "https://app.example.test"
	t.Cleanup(func() { testHandler.cfg = originalConfig })

	createWriter := httptest.NewRecorder()
	testHandler.CreateDeviceAuthorization(createWriter, newRequest("POST", "/api/auth/device/code", map[string]any{
		"client_name": "expired-device-test",
	}))
	if createWriter.Code != http.StatusOK {
		t.Fatalf("create status = %d: %s", createWriter.Code, createWriter.Body.String())
	}
	var created DeviceAuthorizationCodeResponse
	if err := json.NewDecoder(createWriter.Body).Decode(&created); err != nil {
		t.Fatalf("decode create response: %v", err)
	}
	row, err := testHandler.Queries.GetDeviceAuthorizationByDeviceCodeHash(context.Background(), auth.HashToken(created.DeviceCode))
	if err != nil {
		t.Fatalf("load device authorization: %v", err)
	}
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(), "DELETE FROM device_authorization WHERE id = $1", row.ID)
	})
	if _, err := testPool.Exec(context.Background(), "UPDATE device_authorization SET expires_at = now() - interval '1 second' WHERE id = $1", row.ID); err != nil {
		t.Fatalf("expire device authorization: %v", err)
	}

	w := httptest.NewRecorder()
	testHandler.ExchangeDeviceAuthorizationToken(w, newRequest("POST", "/api/auth/device/token", map[string]any{
		"device_code": created.DeviceCode,
	}))
	if w.Code != http.StatusBadRequest || !strings.Contains(w.Body.String(), `"expired_token"`) {
		t.Fatalf("expired exchange = %d: %s, want expired_token", w.Code, w.Body.String())
	}
}
