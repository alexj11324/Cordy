package handler

import (
	"crypto/subtle"
	"encoding/json"
	"errors"
	"net/http"
	"net/mail"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/auth"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

type emailChangeRequest struct {
	Email string `json:"email"`
	Code  string `json:"code"`
}

var setEmailChangeAuthCookies = auth.SetAuthCookies

func decodeEmailChangeRequest(w http.ResponseWriter, r *http.Request) (emailChangeRequest, bool) {
	var request emailChangeRequest
	if json.NewDecoder(r.Body).Decode(&request) != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return request, false
	}
	request.Email = strings.ToLower(strings.TrimSpace(request.Email))
	request.Code = strings.TrimSpace(request.Code)
	parsed, err := mail.ParseAddress(request.Email)
	if err != nil || parsed.Address != request.Email || len(request.Email) > 254 || !strings.Contains(request.Email, "@") {
		writeError(w, http.StatusBadRequest, "invalid email")
		return request, false
	}
	if auth.IsTemporarilyDisabledUserEmail(request.Email) {
		writeError(w, http.StatusForbidden, auth.TemporarilyDisabledUserError)
		return request, false
	}
	return request, true
}

// A verified destination is required before the account's login address changes.
// Codes are bound to both the signed-in account and a separate email-change purpose.
func (h *Handler) RequestEmailChange(w http.ResponseWriter, r *http.Request) {
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	request, ok := decodeEmailChangeRequest(w, r)
	if !ok {
		return
	}
	if h.TxStarter == nil || h.EmailService == nil {
		writeError(w, http.StatusServiceUnavailable, "email change is unavailable")
		return
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to start email change")
		return
	}
	defer tx.Rollback(r.Context())
	id := parseUUID(userID)
	if _, err := tx.Exec(r.Context(), `SELECT id FROM "user" WHERE id=$1 FOR UPDATE`, id); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to lock account")
		return
	}
	q := h.Queries.WithTx(tx)
	user, err := q.GetUser(r.Context(), id)
	if err != nil {
		writeError(w, http.StatusNotFound, "user not found")
		return
	}
	if user.IsGuest {
		writeError(w, http.StatusForbidden, "sign in before changing email")
		return
	}
	if request.Email == user.Email {
		writeError(w, http.StatusBadRequest, "enter a different email")
		return
	}
	if _, err := q.GetUserByEmail(r.Context(), request.Email); err == nil {
		writeError(w, http.StatusConflict, "email is already in use")
		return
	} else if !errors.Is(err, pgx.ErrNoRows) {
		writeError(w, http.StatusInternalServerError, "failed to check email")
		return
	}
	latest, err := q.GetLatestEmailChangeRequest(r.Context(), id)
	if err == nil && time.Since(latest.Time) < time.Minute {
		writeError(w, http.StatusTooManyRequests, "please wait before requesting another code")
		return
	}
	if err != nil && !errors.Is(err, pgx.ErrNoRows) {
		writeError(w, http.StatusInternalServerError, "failed to check email change request")
		return
	}
	code, err := generateCode()
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to generate verification code")
		return
	}
	if err := q.CreateEmailChangeVerificationCode(r.Context(), db.CreateEmailChangeVerificationCodeParams{Email: request.Email, Code: code, ExpiresAt: pgtype.Timestamptz{Time: time.Now().Add(10 * time.Minute), Valid: true}, RequesterUserID: id}); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to store verification code")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to save email change request")
		return
	}
	if err := h.EmailService.SendVerificationCode(request.Email, code); err != nil {
		writeError(w, http.StatusBadGateway, "failed to send verification code")
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"message": "Verification code sent"})
}

func (h *Handler) ConfirmEmailChange(w http.ResponseWriter, r *http.Request) {
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	request, ok := decodeEmailChangeRequest(w, r)
	if !ok {
		return
	}
	if request.Code == "" {
		writeError(w, http.StatusBadRequest, "verification code is required")
		return
	}
	if h.TxStarter == nil {
		writeError(w, http.StatusServiceUnavailable, "email change is unavailable")
		return
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to start email change")
		return
	}
	defer tx.Rollback(r.Context())
	id := parseUUID(userID)
	if _, err := tx.Exec(r.Context(), `SELECT id FROM "user" WHERE id=$1 FOR UPDATE`, id); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to lock account")
		return
	}
	q := h.Queries.WithTx(tx)
	user, err := q.GetUser(r.Context(), id)
	if err != nil {
		writeError(w, http.StatusNotFound, "user not found")
		return
	}
	if user.IsGuest {
		writeError(w, http.StatusForbidden, "sign in before changing email")
		return
	}
	code, err := q.GetEmailChangeVerificationCode(r.Context(), db.GetEmailChangeVerificationCodeParams{RequesterUserID: id, Email: request.Email})
	if errors.Is(err, pgx.ErrNoRows) {
		writeError(w, http.StatusBadRequest, "invalid or expired code")
		return
	}
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to load verification code")
		return
	}
	if subtle.ConstantTimeCompare([]byte(code.Code), []byte(request.Code)) != 1 {
		if err := q.IncrementVerificationCodeAttempts(r.Context(), code.ID); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to verify code")
			return
		}
		if err := tx.Commit(r.Context()); err != nil {
			writeError(w, http.StatusInternalServerError, "failed to verify code")
			return
		}
		writeError(w, http.StatusBadRequest, "invalid or expired code")
		return
	}
	updated, err := q.UpdateUserEmail(r.Context(), db.UpdateUserEmailParams{ID: id, Email: request.Email})
	if err != nil {
		var constraint *pgconn.PgError
		if errors.As(err, &constraint) && constraint.Code == "23505" {
			writeError(w, http.StatusConflict, "email is already in use")
			return
		}
		writeError(w, http.StatusInternalServerError, "failed to update email")
		return
	}
	if err := q.MarkVerificationCodeUsed(r.Context(), code.ID); err != nil {
		writeError(w, http.StatusInternalServerError, "failed to consume verification code")
		return
	}
	token, err := h.issueJWT(updated)
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to refresh session")
		return
	}
	if err := setEmailChangeAuthCookies(w, token); err != nil {
		auth.ClearAuthCookies(w)
		writeError(w, http.StatusInternalServerError, "failed to refresh session")
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		auth.ClearAuthCookies(w)
		writeError(w, http.StatusInternalServerError, "failed to save email change")
		return
	}
	writeJSON(w, http.StatusOK, LoginResponse{Token: token, User: h.userToResponse(updated)})
}
