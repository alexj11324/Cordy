package handler

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/integrations/lark"
	"github.com/patchbay-ai/patchbay/server/internal/integrations/wecom"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestConnectionProjectionRequiresCurrentLeaseAndFreshObservation(t *testing.T) {
	now := time.Date(2026, time.September, 3, 12, 0, 0, 0, time.UTC)
	base := db.ListChannelConnectionStatesRow{
		Status: "installed", UpdatedAt: pgtype.Timestamptz{Time: now.Add(-time.Hour), Valid: true},
		State: pgtype.Text{String: "healthy", Valid: true},
		ObservedAt: pgtype.Timestamptz{Time: now.Add(-time.Second), Valid: true},
		ObserverToken: pgtype.Text{String: "current", Valid: true},
		WsLeaseToken: pgtype.Text{String: "current", Valid: true},
		WsLeaseExpiresAt: pgtype.Timestamptz{Time: now.Add(time.Minute), Valid: true},
	}
	for _, tc := range []struct {
		name string
		change func(*db.ListChannelConnectionStatesRow)
		state string
		code string
	}{
		{"confirmed", func(*db.ListChannelConnectionStatesRow) {}, "healthy", ""},
		{"expired", func(r *db.ListChannelConnectionStatesRow) { r.WsLeaseExpiresAt.Time = now }, "offline", "lease_expired"},
		{"rotated", func(r *db.ListChannelConnectionStatesRow) { r.WsLeaseToken.String = "successor" }, "offline", "lease_generation_mismatch"},
		{"revoked", func(r *db.ListChannelConnectionStatesRow) { r.Status = "revoked" }, "offline", "installation_revoked"},
		{"paused", func(r *db.ListChannelConnectionStatesRow) { r.HostedPausedAt = r.ObservedAt }, "offline", "hosted_quota_paused"},
		{"unobserved lease", func(r *db.ListChannelConnectionStatesRow) { r.ObservedAt.Valid = false }, "starting", ""},
		{"unobserved idle", func(r *db.ListChannelConnectionStatesRow) { r.ObservedAt.Valid = false; r.WsLeaseExpiresAt.Valid = false }, "offline", "runtime_unobserved"},
		{"managed fresh", func(r *db.ListChannelConnectionStatesRow) { r.ObserverToken.String = "managed:slack:webhook:v1"; r.WsLeaseExpiresAt.Valid = false }, "healthy", ""},
		{"managed stale", func(r *db.ListChannelConnectionStatesRow) { r.ObserverToken.String = "managed:slack:webhook:v1"; r.ObservedAt.Time = now.Add(-16*time.Minute) }, "offline", "health_observation_stale"},
		{"future timestamp", func(r *db.ListChannelConnectionStatesRow) { r.ObservedAt.Time = now.Add(time.Minute) }, "offline", "health_observation_stale"},
		{"unknown state", func(r *db.ListChannelConnectionStatesRow) { r.State.String = "future_state" }, "future_state", ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			row := base
			tc.change(&row)
			got := projectConnectionStatus(row, now)
			code := ""
			if got.ErrorCode != nil {
				code = *got.ErrorCode
			}
			if got.State != tc.state || code != tc.code || got.ErrorSummary != nil {
				t.Fatalf("unexpected public connection projection: %+v / %s", got, code)
			}
		})
	}
}

func TestConnectionProjectionUsesTheConfiguredLeaseAuthority(t *testing.T) {
	now := time.Date(2026, time.September, 3, 12, 0, 0, 0, time.UTC)
	row := db.ListChannelConnectionStatesRow{
		Status:        "installed",
		UpdatedAt:     pgtype.Timestamptz{Time: now.Add(-time.Hour), Valid: true},
		State:         pgtype.Text{String: "healthy", Valid: true},
		ObservedAt:    pgtype.Timestamptz{Time: now.Add(-time.Second), Valid: true},
		ObserverToken: pgtype.Text{String: "redis-current", Valid: true},
	}
	for _, tc := range []struct {
		name  string
		lease authoritativeChannelLease
		state string
		code  string
	}{
		{"matching Redis owner", authoritativeChannelLease{Alive: true, Token: "redis-current"}, "healthy", ""},
		{"missing Redis owner", authoritativeChannelLease{}, "offline", "lease_expired"},
		{"successor Redis owner", authoritativeChannelLease{Alive: true, Token: "redis-successor"}, "offline", "lease_generation_mismatch"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := projectConnectionStatusWithLease(row, now, &tc.lease)
			code := ""
			if got.ErrorCode != nil {
				code = *got.ErrorCode
			}
			if got.State != tc.state || code != tc.code {
				t.Fatalf("projection = %+v / %q, want %s / %q", got, code, tc.state, tc.code)
			}
		})
	}
}

func TestRevokedInstallationsExposeDisconnectedStatus(t *testing.T) {
	row := db.ChannelInstallation{Status: "revoked", Config: []byte(`{}`)}
	for name, response := range map[string]any{
		"slack":    slackInstallationToResponse(row),
		"dingtalk": dingtalkInstallationToResponse(row),
		"telegram": telegramInstallationToResponse(row),
		"weixin":   weixinInstallationToResponse(row),
		"lark":     larkInstallationToResponse(lark.Installation{Status: "revoked"}),
		"wecom":    wecomInstallationToResponse(wecom.Installation{Status: "revoked"}),
	} {
		t.Run(name, func(t *testing.T) {
			encoded, err := json.Marshal(response)
			if err != nil {
				t.Fatal(err)
			}
			var got struct {
				Runtime struct {
					State     string `json:"state"`
					ErrorCode string `json:"errorCode"`
				} `json:"runtime"`
			}
			if err := json.Unmarshal(encoded, &got); err != nil {
				t.Fatal(err)
			}
			if got.Runtime.State != "offline" || got.Runtime.ErrorCode != "installation_revoked" {
				t.Fatalf("revoked installation omitted its authoritative disconnected state: %s", encoded)
			}
		})
	}
}

func TestInstalledResponsesPreserveLegacyActiveAndExposeCanonicalStatus(t *testing.T) {
	row := db.ChannelInstallation{Status: "installed", Config: []byte(`{}`)}
	for name, response := range map[string]any{
		"slack":    slackInstallationToResponse(row),
		"dingtalk": dingtalkInstallationToResponse(row),
		"telegram": telegramInstallationToResponse(row),
		"weixin":   weixinInstallationToResponse(row),
		"lark":     larkInstallationToResponse(lark.Installation{Status: "installed"}),
		"wecom":    wecomInstallationToResponse(wecom.Installation{Status: "installed"}),
	} {
		t.Run(name, func(t *testing.T) {
			encoded, err := json.Marshal(response)
			if err != nil {
				t.Fatal(err)
			}
			var got struct {
				Status             string `json:"status"`
				InstallationStatus string `json:"installation_status"`
			}
			if err := json.Unmarshal(encoded, &got); err != nil {
				t.Fatal(err)
			}
			if got.Status != "active" || got.InstallationStatus != "installed" {
				t.Fatalf("installation compatibility fields = %+v, want active/installed: %s", got, encoded)
			}
		})
	}
}
