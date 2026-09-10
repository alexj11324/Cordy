package handler

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"net/url"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/orvilo-ai/orvilo/server/internal/auth"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

const (
	deviceAuthorizationTTL              = 10 * time.Minute
	deviceAuthorizationPollInterval     = 5
	deviceAuthorizationMaxPolls         = 180
	deviceAuthorizationMaxClientName    = 120
	deviceAuthorizationUserCodeLength   = 8
	deviceAuthorizationCodePrefix       = "odc_"
	deviceAuthorizationUserCodeAlphabet = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"
)

// Device authorization follows RFC 8628's public device-code and
// authenticated user-approval split. The raw codes are returned only from
// CreateDeviceAuthorization and are persisted as hashes.
type DeviceAuthorizationCodeRequest struct {
	ClientName string `json:"client_name"`
}

type DeviceAuthorizationCodeResponse struct {
	DeviceCode      string `json:"device_code"`
	UserCode        string `json:"user_code"`
	VerificationURI string `json:"verification_uri"`
	ExpiresIn       int    `json:"expires_in"`
	Interval        int    `json:"interval"`
}

type DeviceAuthorizationTokenRequest struct {
	DeviceCode string `json:"device_code"`
}

type DeviceAuthorizationTokenResponse struct {
	AccessToken string `json:"access_token"`
	TokenType   string `json:"token_type"`
}

type DeviceAuthorizationInspectRequest struct {
	UserCode string `json:"user_code"`
}

type DeviceAuthorizationInspectResponse struct {
	ClientName string `json:"client_name"`
	ExpiresAt  string `json:"expires_at"`
}

type DeviceAuthorizationDecisionRequest struct {
	UserCode string `json:"user_code"`
	Approve  *bool  `json:"approve"`
}

type deviceAuthorizationTokenError struct {
	Error            string `json:"error"`
	ErrorDescription string `json:"error_description,omitempty"`
}

// User codes deliberately omit characters that are easy to confuse when read
// from a terminal. The server accepts the displayed XXXX-XXXX form and the
// compact form so users can paste either one into the browser.
func generateDeviceUserCode() (string, error) {
	const alphabet = deviceAuthorizationUserCodeAlphabet
	var raw [deviceAuthorizationUserCodeLength]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return "", err
	}

	// Rejection sampling avoids modulo bias while keeping the helper bounded.
	// The alphabet length is 30, so fewer than 2% of bytes are rejected.
	limit := 256 - (256 % len(alphabet))
	for i := range raw {
		for int(raw[i]) >= limit {
			if _, err := rand.Read(raw[i : i+1]); err != nil {
				return "", err
			}
		}
		raw[i] = alphabet[int(raw[i])%len(alphabet)]
	}
	return formatDeviceUserCode(string(raw[:])), nil
}

func generateDeviceCode() (string, error) {
	var raw [32]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return "", err
	}
	return deviceAuthorizationCodePrefix + base64.RawURLEncoding.EncodeToString(raw[:]), nil
}

func formatDeviceUserCode(raw string) string {
	if len(raw) != deviceAuthorizationUserCodeLength {
		return raw
	}
	return raw[:4] + "-" + raw[4:]
}

func normalizeDeviceUserCode(raw string) string {
	var b strings.Builder
	b.Grow(len(raw))
	for _, r := range strings.ToUpper(strings.TrimSpace(raw)) {
		if r == '-' || r == ' ' || r == '\t' || r == '\n' || r == '\r' {
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}

func validDeviceUserCode(raw string) bool {
	if len(raw) != deviceAuthorizationUserCodeLength {
		return false
	}
	for _, r := range raw {
		if !strings.ContainsRune(deviceAuthorizationUserCodeAlphabet, r) {
			return false
		}
	}
	return true
}

func validDeviceClientName(raw string) (string, bool) {
	name := strings.TrimSpace(raw)
	if name == "" || !utf8.ValidString(name) || utf8.RuneCountInString(name) > deviceAuthorizationMaxClientName {
		return "", false
	}
	return name, true
}

func deviceAuthorizationVerificationURI(h *Handler) string {
	base := strings.TrimRight(strings.TrimSpace(h.cfg.AppURL), "/")
	if base == "" {
		base = strings.TrimRight(strings.TrimSpace(h.cfg.PublicURL), "/")
	}
	if base == "" {
		return ""
	}
	return base + "/device"
}

func validDeviceVerificationURI(raw string) bool {
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		return false
	}
	return u.Scheme == "http" || u.Scheme == "https"
}

func deviceAuthorizationExpiresAt(now time.Time) pgtype.Timestamptz {
	return pgtype.Timestamptz{Time: now.Add(deviceAuthorizationTTL), Valid: true}
}

func deviceAuthorizationIsUsable(row db.DeviceAuthorization, now time.Time) bool {
	return row.Status == "pending" && row.ExpiresAt.Valid && row.ExpiresAt.Time.After(now) && row.PollCount < deviceAuthorizationMaxPolls
}

func writeDeviceAuthorizationError(w http.ResponseWriter, status int, code, description string) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Pragma", "no-cache")
	writeJSON(w, status, deviceAuthorizationTokenError{Error: code, ErrorDescription: description})
}

func requireDeviceAuthorizationActor(w http.ResponseWriter, r *http.Request) bool {
	// Guest sessions can call ordinary workspace APIs, but they are not a
	// formal account and must not turn a temporary guest credential into a
	// long-lived personal access token. Auth middleware owns this marker and
	// strips any client-supplied value before setting it.
	if isMachineCredentialActor(r) || r.Header.Get("X-Guest-User") == "true" {
		writeError(w, http.StatusForbidden, "device authorization requires a formal user")
		return false
	}
	return true
}

func decodeJSONBody(r *http.Request, dst any) error {
	decoder := json.NewDecoder(r.Body)
	if err := decoder.Decode(dst); err != nil {
		return err
	}
	return nil
}

// CreateDeviceAuthorization starts a headless CLI login. It does not require
// an Orvilo session: possession of the device code only permits polling, and
// the browser must separately authenticate before the code can be approved.
func (h *Handler) CreateDeviceAuthorization(w http.ResponseWriter, r *http.Request) {
	var req DeviceAuthorizationCodeRequest
	if err := decodeJSONBody(r, &req); err != nil {
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "invalid_request", "invalid request body")
		return
	}
	clientName, ok := validDeviceClientName(req.ClientName)
	if !ok {
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "invalid_request", "client_name is required and must be at most 120 characters")
		return
	}
	verificationURI := deviceAuthorizationVerificationURI(h)
	if !validDeviceVerificationURI(verificationURI) {
		writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is not configured for this server")
		return
	}

	deviceCode, err := generateDeviceCode()
	if err != nil {
		writeDeviceAuthorizationError(w, http.StatusInternalServerError, "server_error", "failed to generate device code")
		return
	}
	userCode, err := generateDeviceUserCode()
	if err != nil {
		writeDeviceAuthorizationError(w, http.StatusInternalServerError, "server_error", "failed to generate user code")
		return
	}

	row, err := h.Queries.CreateDeviceAuthorization(r.Context(), db.CreateDeviceAuthorizationParams{
		ClientName:      clientName,
		DeviceCodeHash:  auth.HashToken(deviceCode),
		UserCodeHash:    auth.HashToken(normalizeDeviceUserCode(userCode)),
		ExpiresAt:       deviceAuthorizationExpiresAt(time.Now()),
		IntervalSeconds: deviceAuthorizationPollInterval,
	})
	if err != nil {
		writeDeviceAuthorizationError(w, http.StatusInternalServerError, "server_error", "failed to create device authorization")
		return
	}
	writeJSON(w, http.StatusOK, DeviceAuthorizationCodeResponse{
		DeviceCode:      deviceCode,
		UserCode:        formatDeviceUserCode(normalizeDeviceUserCode(userCode)),
		VerificationURI: verificationURI,
		ExpiresIn:       int(deviceAuthorizationTTL.Seconds()),
		Interval:        int(row.IntervalSeconds),
	})
}

// InspectDeviceAuthorization returns the pending client name to an
// authenticated browser so the user can confirm which CLI is requesting
// access. It intentionally does not bind or approve the request.
func (h *Handler) InspectDeviceAuthorization(w http.ResponseWriter, r *http.Request) {
	if !requireDeviceAuthorizationActor(w, r) {
		return
	}
	if _, ok := requireUserID(w, r); !ok {
		return
	}
	var req DeviceAuthorizationInspectRequest
	if err := decodeJSONBody(r, &req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	userCode := normalizeDeviceUserCode(req.UserCode)
	if !validDeviceUserCode(userCode) {
		writeError(w, http.StatusBadRequest, "invalid or expired device code")
		return
	}

	row, err := h.Queries.GetDeviceAuthorizationByUserCodeHash(r.Context(), auth.HashToken(userCode))
	if err != nil || !deviceAuthorizationIsUsable(row, time.Now()) {
		writeError(w, http.StatusBadRequest, "invalid or expired device code")
		return
	}
	writeJSON(w, http.StatusOK, DeviceAuthorizationInspectResponse{
		ClientName: row.ClientName,
		ExpiresAt:  timestampToString(row.ExpiresAt),
	})
}

// DecideDeviceAuthorization binds the authenticated browser user to a
// pending device request. Approval and denial are both one-way transitions.
func (h *Handler) DecideDeviceAuthorization(w http.ResponseWriter, r *http.Request) {
	if !requireDeviceAuthorizationActor(w, r) {
		return
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return
	}
	var req DeviceAuthorizationDecisionRequest
	if err := decodeJSONBody(r, &req); err != nil || req.Approve == nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return
	}
	userCode := normalizeDeviceUserCode(req.UserCode)
	if !validDeviceUserCode(userCode) {
		writeError(w, http.StatusBadRequest, "invalid or expired device code")
		return
	}

	row, err := h.Queries.GetDeviceAuthorizationByUserCodeHash(r.Context(), auth.HashToken(userCode))
	if err != nil || !deviceAuthorizationIsUsable(row, time.Now()) {
		writeError(w, http.StatusBadRequest, "invalid or expired device code")
		return
	}

	if *req.Approve {
		_, err = h.Queries.ApproveDeviceAuthorization(r.Context(), db.ApproveDeviceAuthorizationParams{
			ID:        row.ID,
			UserID:    parseUUID(userID),
			PollCount: deviceAuthorizationMaxPolls,
		})
	} else {
		_, err = h.Queries.DenyDeviceAuthorization(r.Context(), db.DenyDeviceAuthorizationParams{
			ID:        row.ID,
			PollCount: deviceAuthorizationMaxPolls,
		})
	}
	if errors.Is(err, pgx.ErrNoRows) {
		writeError(w, http.StatusConflict, "device authorization is no longer pending")
		return
	}
	if err != nil {
		writeError(w, http.StatusInternalServerError, "failed to update device authorization")
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func commitDeviceAuthorizationPoll(ctx context.Context, tx pgx.Tx, qtx *db.Queries, row db.DeviceAuthorization) error {
	if _, err := qtx.RecordDeviceAuthorizationPoll(ctx, row.ID); err != nil {
		return err
	}
	if err := tx.Commit(ctx); err != nil {
		return err
	}
	return nil
}

func handlePendingDeviceAuthorization(w http.ResponseWriter, r *http.Request, tx pgx.Tx, qtx *db.Queries, row db.DeviceAuthorization, now time.Time) bool {
	if row.PollCount >= deviceAuthorizationMaxPolls {
		if _, err := qtx.MarkDeviceAuthorizationExpired(r.Context(), row.ID); err != nil {
			writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
			return false
		}
		if err := tx.Commit(r.Context()); err != nil {
			writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
			return false
		}
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "expired_token", "device code polling limit exceeded")
		return true
	}

	code, description := "authorization_pending", "authorization is pending"
	if row.LastPolledAt.Valid && now.Sub(row.LastPolledAt.Time) < time.Duration(row.IntervalSeconds)*time.Second {
		// Count too-fast polls as attempts as well. This keeps a stolen device
		// code from being polled indefinitely while returning RFC 8628's signal.
		code, description = "slow_down", "poll less frequently"
	}
	if err := commitDeviceAuthorizationPoll(r.Context(), tx, qtx, row); err != nil {
		writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
		return false
	}
	writeDeviceAuthorizationError(w, http.StatusBadRequest, code, description)
	return true
}

func writeDeviceAuthorizationStateError(w http.ResponseWriter, row db.DeviceAuthorization, now time.Time) bool {
	switch {
	case row.Status == "denied":
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "access_denied", "the user denied the authorization request")
		return true
	case row.Status == "consumed" || row.Status == "expired" || !row.ExpiresAt.Valid || !row.ExpiresAt.Time.After(now):
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "expired_token", "device code is invalid or expired")
		return true
	case row.Status != "pending" && row.Status != "approved":
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "expired_token", "device code is invalid or expired")
		return true
	default:
		return false
	}
}

func (h *Handler) issueDeviceAuthorizationToken(w http.ResponseWriter, r *http.Request, tx pgx.Tx, qtx *db.Queries, row db.DeviceAuthorization, now time.Time) bool {
	if !row.UserID.Valid {
		writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
		return false
	}
	rawToken, err := auth.GeneratePATToken()
	if err != nil {
		writeDeviceAuthorizationError(w, http.StatusInternalServerError, "server_error", "failed to generate access token")
		return false
	}
	if _, err := qtx.CreatePersonalAccessToken(r.Context(), db.CreatePersonalAccessTokenParams{
		UserID:      row.UserID,
		Name:        row.ClientName,
		TokenHash:   auth.HashToken(rawToken),
		TokenPrefix: rawToken[:min(12, len(rawToken))],
		ExpiresAt:   pgtype.Timestamptz{Time: now.Add(PATRenewExtension), Valid: true},
	}); err != nil {
		writeDeviceAuthorizationError(w, http.StatusInternalServerError, "server_error", "failed to create access token")
		return false
	}
	if _, err := qtx.ConsumeDeviceAuthorization(r.Context(), db.ConsumeDeviceAuthorizationParams{ID: row.ID, UserID: row.UserID}); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			writeDeviceAuthorizationError(w, http.StatusBadRequest, "expired_token", "device code is invalid or expired")
			return false
		}
		writeDeviceAuthorizationError(w, http.StatusInternalServerError, "server_error", "failed to finalize device authorization")
		return false
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
		return false
	}
	writeJSON(w, http.StatusOK, DeviceAuthorizationTokenResponse{AccessToken: rawToken, TokenType: "Bearer"})
	return true
}

// ExchangeDeviceAuthorizationToken implements the polling side of RFC 8628.
// The approved request and PAT insertion are committed together, so a device
// code can produce at most one PAT even when two pollers race.
func (h *Handler) ExchangeDeviceAuthorizationToken(w http.ResponseWriter, r *http.Request) {
	var req DeviceAuthorizationTokenRequest
	if err := decodeJSONBody(r, &req); err != nil {
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "invalid_request", "invalid request body")
		return
	}
	deviceCode := strings.TrimSpace(req.DeviceCode)
	if deviceCode == "" {
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "invalid_request", "device_code is required")
		return
	}
	if h.TxStarter == nil {
		writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
		return
	}

	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
		return
	}
	committed := false
	defer func() {
		if !committed {
			_ = tx.Rollback(context.Background())
		}
	}()

	qtx := h.Queries.WithTx(tx)
	row, err := qtx.GetDeviceAuthorizationByDeviceCodeHashForUpdate(r.Context(), auth.HashToken(deviceCode))
	if errors.Is(err, pgx.ErrNoRows) {
		writeDeviceAuthorizationError(w, http.StatusBadRequest, "expired_token", "device code is invalid or expired")
		return
	}
	if err != nil {
		writeDeviceAuthorizationError(w, http.StatusServiceUnavailable, "temporarily_unavailable", "device authorization is temporarily unavailable")
		return
	}

	now := time.Now()
	if writeDeviceAuthorizationStateError(w, row, now) {
		return
	}
	if row.Status == "pending" {
		committed = handlePendingDeviceAuthorization(w, r, tx, qtx, row, now)
		return
	}
	if row.Status == "approved" {
		committed = h.issueDeviceAuthorizationToken(w, r, tx, qtx, row, now)
	}
}
