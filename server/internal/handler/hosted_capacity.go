package handler

import (
	"errors"
	"net/http"

	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/hostedcapacity"
)

// Hosted-installation-capacity error surface, shared by every install
// endpoint. The codes are stable so a UI can translate the failure instead of
// toasting the English sentence.
const (
	hostedCapacityLimitCode    = "im_installation_limit_reached"
	hostedCapacityLimitMessage = "this workspace has reached its hosted messaging installation limit"
	hostedCapacityQuotaCode    = "im_installation_quota_unavailable"
	hostedCapacityQuotaMessage = "hosted messaging installation quota is temporarily unavailable"
)

// hostedInstallationLimit resolves the workspace's hosted installation cap and
// reconciles the durable pause markers to match. It returns (limit, true) on
// success — a nil limit means no admission check — and (nil, false) after
// writing the fail-closed 503 itself, so callers just `if !ok { return }`.
//
// Resolving at the ENDPOINT (before any token exchange, credential probe, or
// QR issuance) is deliberate: side effects that burn single-use credentials
// must not run when the install could never be persisted. The persist layer
// re-enforces the limit under the workspace lock, closing the TOCTOU window
// between this resolution and the upsert.
func (h *Handler) hostedInstallationLimit(w http.ResponseWriter, r *http.Request, workspaceID pgtype.UUID) (limit *int64, ok bool) {
	if h.HostedCapacity == nil {
		return nil, true
	}
	limit, err := h.HostedCapacity.InstallationLimit(r.Context(), workspaceID)
	if err != nil {
		writeErrorCode(w, http.StatusServiceUnavailable, hostedCapacityQuotaCode, hostedCapacityQuotaMessage)
		return nil, false
	}
	return limit, true
}

// writeHostedCapacityError maps the admission sentinels onto the shared error
// surface. Returns true when the error was one of ours and the response is
// written; false when the caller should keep classifying.
func writeHostedCapacityError(w http.ResponseWriter, err error) bool {
	switch {
	case err == nil:
		return false
	case errors.Is(err, hostedcapacity.ErrLimitReached):
		writeErrorCode(w, http.StatusForbidden, hostedCapacityLimitCode, hostedCapacityLimitMessage)
		return true
	case errors.Is(err, hostedcapacity.ErrQuotaUnavailable):
		writeErrorCode(w, http.StatusServiceUnavailable, hostedCapacityQuotaCode, hostedCapacityQuotaMessage)
		return true
	default:
		return false
	}
}
