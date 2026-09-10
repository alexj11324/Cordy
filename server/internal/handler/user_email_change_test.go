package handler

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/service"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

func newEmailChangeTestUser(t *testing.T, email string) string {
	t.Helper()
	id := newLanguageTestUser(t, email)
	t.Cleanup(func() {
		_, _ = testPool.Exec(context.Background(),
			`DELETE FROM verification_code WHERE requester_user_id = $1`, id)
	})
	return id
}

func newEmailChangeHTTPRequest(userID, path, body string) *http.Request {
	req := httptest.NewRequest(http.MethodPost, path, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-User-ID", userID)
	return req
}

func useDevEmailService(t *testing.T) {
	t.Helper()
	previous := testHandler.EmailService
	testHandler.EmailService = &service.EmailService{}
	t.Cleanup(func() { testHandler.EmailService = previous })
}

func TestEmailChangeRequestAndConfirmUseBoundSinglePurposeCode(t *testing.T) {
	useDevEmailService(t)
	ownerID := newEmailChangeTestUser(t, "email-change-owner@orvilo.ai")
	intruderID := newEmailChangeTestUser(t, "email-change-intruder@orvilo.ai")
	target := "email-change-target@orvilo.ai"

	w := httptest.NewRecorder()
	testHandler.RequestEmailChange(w, newEmailChangeHTTPRequest(ownerID, "/api/me/email-change", `{"email":"EMAIL-CHANGE-TARGET@orvilo.ai"}`))
	if w.Code != http.StatusOK {
		t.Fatalf("request: %d %s", w.Code, w.Body.String())
	}

	var code, purpose, requesterID string
	if err := testPool.QueryRow(context.Background(), `
		SELECT code, purpose, requester_user_id::text
		FROM verification_code
		WHERE requester_user_id = $1 AND email = $2
		ORDER BY created_at DESC LIMIT 1`, ownerID, target,
	).Scan(&code, &purpose, &requesterID); err != nil {
		t.Fatalf("load stored challenge: %v", err)
	}
	if purpose != "email_change" || requesterID != ownerID || len(code) != 6 {
		t.Fatalf("challenge was not scoped correctly: purpose=%q requester=%q code_len=%d", purpose, requesterID, len(code))
	}
	if _, err := testHandler.Queries.GetLatestVerificationCode(context.Background(), target); !errors.Is(err, pgx.ErrNoRows) {
		t.Fatalf("email-change code was visible to login verification: %v", err)
	}

	w = httptest.NewRecorder()
	testHandler.ConfirmEmailChange(w, newEmailChangeHTTPRequest(intruderID, "/api/me/email-change/confirm", `{"email":"email-change-target@orvilo.ai","code":"`+code+`"}`))
	if w.Code != http.StatusBadRequest {
		t.Fatalf("other account reused challenge: %d %s", w.Code, w.Body.String())
	}

	w = httptest.NewRecorder()
	testHandler.ConfirmEmailChange(w, newEmailChangeHTTPRequest(ownerID, "/api/me/email-change/confirm", `{"email":"email-change-target@orvilo.ai","code":"000000"}`))
	if w.Code != http.StatusBadRequest {
		t.Fatalf("wrong code: %d %s", w.Code, w.Body.String())
	}
	var attempts int
	if err := testPool.QueryRow(context.Background(),
		`SELECT attempts FROM verification_code WHERE requester_user_id = $1 AND email = $2`, ownerID, target,
	).Scan(&attempts); err != nil {
		t.Fatalf("load attempts: %v", err)
	}
	if attempts != 1 {
		t.Fatalf("wrong code attempts=%d, want 1", attempts)
	}

	w = httptest.NewRecorder()
	testHandler.ConfirmEmailChange(w, newEmailChangeHTTPRequest(ownerID, "/api/me/email-change/confirm", `{"email":"email-change-target@orvilo.ai","code":"`+code+`"}`))
	if w.Code != http.StatusOK {
		t.Fatalf("confirm: %d %s", w.Code, w.Body.String())
	}
	if !strings.Contains(w.Body.String(), `"email":"email-change-target@orvilo.ai"`) || !strings.Contains(w.Body.String(), `"token":`) {
		t.Fatalf("confirm response omitted refreshed identity: %s", w.Body.String())
	}
	cookies := w.Result().Cookies()
	if len(cookies) < 2 {
		t.Fatalf("confirm did not refresh auth and CSRF cookies: %v", cookies)
	}
	var email string
	var used bool
	if err := testPool.QueryRow(context.Background(), `
		SELECT u.email, v.used
		FROM "user" u
		JOIN verification_code v ON v.requester_user_id = u.id
		WHERE u.id = $1 AND v.email = $2`, ownerID, target,
	).Scan(&email, &used); err != nil {
		t.Fatalf("load confirmed state: %v", err)
	}
	if email != target || !used {
		t.Fatalf("confirm did not atomically update account and consume challenge: email=%q used=%v", email, used)
	}

	w = httptest.NewRecorder()
	testHandler.ConfirmEmailChange(w, newEmailChangeHTTPRequest(ownerID, "/api/me/email-change/confirm", `{"email":"email-change-target@orvilo.ai","code":"`+code+`"}`))
	if w.Code != http.StatusBadRequest {
		t.Fatalf("consumed challenge was reusable: %d %s", w.Code, w.Body.String())
	}
}

func TestConfirmEmailChangeCookieFailureRollsBack(t *testing.T) {
	ownerID := newEmailChangeTestUser(t, "email-change-cookie-owner@orvilo.ai")
	target := "email-change-cookie-target@orvilo.ai"
	if err := testHandler.Queries.CreateEmailChangeVerificationCode(context.Background(), db.CreateEmailChangeVerificationCodeParams{
		Email:           target,
		Code:            "123456",
		ExpiresAt:       pgtype.Timestamptz{Time: time.Now().Add(10 * time.Minute), Valid: true},
		RequesterUserID: parseUUID(ownerID),
	}); err != nil {
		t.Fatalf("seed challenge: %v", err)
	}

	previous := setEmailChangeAuthCookies
	setEmailChangeAuthCookies = func(http.ResponseWriter, string) error {
		return errors.New("forced cookie failure")
	}
	t.Cleanup(func() { setEmailChangeAuthCookies = previous })

	w := httptest.NewRecorder()
	testHandler.ConfirmEmailChange(w, newEmailChangeHTTPRequest(ownerID, "/api/me/email-change/confirm", `{"email":"email-change-cookie-target@orvilo.ai","code":"123456"}`))
	if w.Code != http.StatusInternalServerError {
		t.Fatalf("cookie failure returned success: %d %s", w.Code, w.Body.String())
	}
	if strings.Contains(w.Body.String(), `"token":`) {
		t.Fatalf("cookie failure leaked a success payload: %s", w.Body.String())
	}

	var email string
	var used bool
	if err := testPool.QueryRow(context.Background(), `
		SELECT u.email, v.used
		FROM "user" u
		JOIN verification_code v ON v.requester_user_id = u.id
		WHERE u.id = $1 AND v.email = $2`, ownerID, target,
	).Scan(&email, &used); err != nil {
		t.Fatalf("load rolled-back state: %v", err)
	}
	if email != "email-change-cookie-owner@orvilo.ai" || used {
		t.Fatalf("cookie failure did not roll back: email=%q used=%v", email, used)
	}
}
