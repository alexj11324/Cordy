package handler

import (
	"context"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// MessagingConnectionStatus is a public projection, not the provider's raw
// diagnostic. In particular, errors and credential-bearing URLs never escape
// through ErrorSummary. Clients localize stable codes and tolerate new states.
type MessagingConnectionStatus struct {
	State        string  `json:"state"`
	ObservedAt   *string `json:"observedAt"`
	ErrorCode    *string `json:"errorCode"`
	ErrorSummary *string `json:"errorSummary"`
}

type ChannelConnectionLeaseReader interface {
	ListLeaseOwners(context.Context, []pgtype.UUID) (map[string]string, error)
}

func connectionStatus(state, code string, observedAt *string) MessagingConnectionStatus {
	result := MessagingConnectionStatus{State: state, ObservedAt: observedAt}
	if code != "" {
		result.ErrorCode = &code
	}
	return result
}

func initialConnectionStatus(status string) MessagingConnectionStatus {
	if status != "installed" {
		return connectionStatus("offline", "installation_revoked", nil)
	}
	return connectionStatus("starting", "", nil)
}

func projectConnectionStatus(row db.ListChannelConnectionStatesRow, now time.Time) MessagingConnectionStatus {
	if row.Status != "installed" {
		return initialConnectionStatus(row.Status)
	}
	if row.HostedPausedAt.Valid {
		return connectionStatus("offline", "hosted_quota_paused", nil)
	}
	leaseAlive := row.WsLeaseExpiresAt.Valid && row.WsLeaseExpiresAt.Time.After(now)
	if !row.ObservedAt.Valid || !row.State.Valid {
		age := now.Sub(row.UpdatedAt.Time)
		if leaseAlive || (row.UpdatedAt.Valid && age >= 0 && age < time.Minute) {
			return initialConnectionStatus(row.Status)
		}
		return connectionStatus("offline", "runtime_unobserved", nil)
	}
	stamp := row.ObservedAt.Time.UTC().Format(time.RFC3339Nano)
	age := now.Sub(row.ObservedAt.Time)
	managed := strings.HasPrefix(row.ObserverToken.String, "managed:")
	control := strings.HasPrefix(row.ObserverToken.String, "control:")
	if age < 0 || (managed && age > 15*time.Minute) {
		return connectionStatus("offline", "health_observation_stale", &stamp)
	}
	if !managed && !control && (row.State.String == "starting" || row.State.String == "healthy") {
		if !leaseAlive {
			return connectionStatus("offline", "lease_expired", &stamp)
		}
		if !row.WsLeaseToken.Valid || !row.ObserverToken.Valid || row.WsLeaseToken.String != row.ObserverToken.String {
			return connectionStatus("offline", "lease_generation_mismatch", &stamp)
		}
	}
	return connectionStatus(row.State.String, row.ErrorCode.String, &stamp)
}

// loadConnectionStatuses batches only IDs already authorized by the list
// handler. The workspace predicate also prevents accidental cross-scope reads.
func (h *Handler) loadConnectionStatuses(ctx context.Context, workspaceID pgtype.UUID, ids []string) (map[string]MessagingConnectionStatus, error) {
	result := make(map[string]MessagingConnectionStatus, len(ids))
	if len(ids) == 0 {
		return result, nil
	}
	uuidIDs := make([]pgtype.UUID, 0, len(ids))
	for _, id := range ids {
		var uuid pgtype.UUID
		if err := uuid.Scan(id); err != nil {
			return nil, err
		}
		uuidIDs = append(uuidIDs, uuid)
		// A concurrent deletion is disconnected, not a new starting attempt.
		result[id] = connectionStatus("offline", "installation_revoked", nil)
	}
	rows, err := h.Queries.ListChannelConnectionStates(ctx, db.ListChannelConnectionStatesParams{
		WorkspaceID: workspaceID, InstallationIds: uuidIDs,
	})
	if err != nil {
		return nil, err
	}
	now := time.Now()
	for _, row := range rows {
		result[uuidToString(row.InstallationID)] = projectConnectionStatus(row, now)
	}
	return result, nil
}
