package slack

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"golang.org/x/sync/errgroup"

	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	managedRefreshAhead        = 30 * time.Minute
	managedRefreshInterval     = 5 * time.Minute
	managedHTTPTimeout         = 15 * time.Second
	managedInstallationTimeout = 40 * time.Second
	managedHealthSweepBudget   = 10 * time.Minute
)

type managedTokenQueries interface {
	ListConnectableManagedSlackInstallations(context.Context) ([]db.ChannelInstallation, error)
	RotateManagedSlackTokens(context.Context, db.RotateManagedSlackTokensParams) (int64, error)
	ObserveManagedSlackRuntime(context.Context, db.ObserveManagedSlackRuntimeParams) (int64, error)
}

// ManagedTokenWorker owns expiring managed credentials and provider health.
// It never handles BYO Socket Mode installs, changes routing identity, or logs
// provider responses/credentials. Run once with the server worker context.
type ManagedTokenWorker struct {
	q           managedTokenQueries
	box         *secretbox.Box
	oauth       *ManagedOAuthService
	authTestURL string
	now         func() time.Time
	logger      *slog.Logger
	done        chan struct{}
}

func NewManagedTokenWorker(q *db.Queries, box *secretbox.Box, oauth *ManagedOAuthService, logger *slog.Logger) *ManagedTokenWorker {
	if q == nil {
		return nil
	}
	return newManagedTokenWorker(q, box, oauth, logger)
}

func newManagedTokenWorker(q managedTokenQueries, box *secretbox.Box, oauth *ManagedOAuthService, logger *slog.Logger) *ManagedTokenWorker {
	if q == nil || box == nil || oauth == nil || strings.TrimSpace(oauth.clientID) == "" || strings.TrimSpace(oauth.clientSecret) == "" {
		return nil
	}
	if logger == nil {
		logger = slog.Default()
	}
	return &ManagedTokenWorker{q: q, box: box, oauth: oauth, logger: logger,
		authTestURL: "https://slack.com/api/auth.test", now: time.Now, done: make(chan struct{})}
}

func (w *ManagedTokenWorker) Run(ctx context.Context) {
	if w == nil {
		return
	}
	defer close(w.done)
	ticker := time.NewTicker(managedRefreshInterval)
	defer ticker.Stop()
	for ctx.Err() == nil {
		// Sweep immediately at boot; a buffered overdue tick starts the next
		// sweep immediately instead of adding another freshness interval.
		if err := w.Sweep(ctx); err != nil && ctx.Err() == nil {
			w.logger.WarnContext(ctx, "managed Slack installation sweep failed")
		}
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

func (w *ManagedTokenWorker) WaitWithTimeout(timeout time.Duration) bool {
	if w == nil {
		return true
	}
	timer := time.NewTimer(timeout)
	defer timer.Stop()
	select {
	case <-w.done:
		return true
	case <-timer.C:
		return false
	}
}

func (w *ManagedTokenWorker) Sweep(ctx context.Context) error {
	rows, err := w.q.ListConnectableManagedSlackInstallations(ctx)
	if err != nil {
		return err
	}
	var group errgroup.Group
	group.SetLimit(managedHealthConcurrency(len(rows)))
	for _, row := range rows {
		if ctx.Err() != nil {
			break
		}
		group.Go(func() error {
			installationCtx, cancel := context.WithTimeout(ctx, managedInstallationTimeout)
			defer cancel()
			w.refreshInstallation(installationCtx, row)
			return nil
		})
	}
	return group.Wait()
}

func managedHealthConcurrency(count int) int {
	// Keep at least eight concurrent probes, then scale the sweep
	// scaled so a large sweep cannot age out early observations before it ends.
	installationsPerSlot := int(managedHealthSweepBudget / managedInstallationTimeout)
	if count <= 0 {
		return 8
	}
	return max(8, 1+(count-1)/installationsPerSlot)
}

func needsManagedTokenRotation(expires *time.Time, now time.Time) bool {
	return expires != nil && !expires.After(now.Add(managedRefreshAhead))
}

func (w *ManagedTokenWorker) refreshInstallation(ctx context.Context, row db.ChannelInstallation) {
	if ctx.Err() != nil || row.Status != "installed" || row.HostedPausedAt.Valid {
		return
	}
	var cfg installConfig
	if err := json.Unmarshal(row.Config, &cfg); err != nil {
		w.logger.WarnContext(ctx, "invalid managed Slack installation config", "installation_id", row.ID)
		return
	}
	if cfg.Transport != ManagedTransportWebhook {
		return
	}
	if cfg.RefreshTokenEncrypted != "" && needsManagedTokenRotation(cfg.TokenExpiresAt, w.now()) {
		rotated, applied, err := w.rotate(ctx, row.ID, cfg)
		if ctx.Err() != nil {
			return
		}
		if err != nil {
			w.logger.WarnContext(ctx, "managed Slack token rotation failed", "installation_id", row.ID)
		} else if !applied {
			// A concurrent reconnect/rotation/pause won. Do not let this old
			// worker publish a stale authentication failure over its successor.
			return
		} else {
			cfg = rotated
		}
	}
	token, err := decryptToken(cfg.BotTokenEncrypted, w.box.Open)
	if err != nil {
		w.recordHealth(ctx, row.ID, cfg, "error", "credential_decryption_failed", "The managed Slack credential could not be read.")
		return
	}
	if token == "" {
		w.recordHealth(ctx, row.ID, cfg, "error", "credential_missing", "The managed Slack access token is missing.")
		return
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, w.authTestURL, nil)
	if err != nil {
		w.recordHealth(ctx, row.ID, cfg, "degraded", "health_probe_failed", "The managed Slack health probe could not reach Slack.")
		return
	}
	req.Header.Set("Authorization", "Bearer "+token)
	body, err := w.request(ctx, req)
	if ctx.Err() != nil {
		return
	}
	if err != nil {
		w.recordHealth(ctx, row.ID, cfg, "degraded", "health_probe_failed", "The managed Slack health probe could not reach Slack.")
		return
	}
	var response struct {
		OK bool `json:"ok"`
	}
	if json.Unmarshal(body, &response) != nil {
		w.recordHealth(ctx, row.ID, cfg, "degraded", "health_probe_invalid_response", "Slack returned an unreadable health response.")
	} else if !response.OK {
		w.recordHealth(ctx, row.ID, cfg, "error", "authentication_failed", "Slack rejected the managed app credential.")
	} else {
		w.recordHealth(ctx, row.ID, cfg, "healthy", "", "")
	}
}

func (w *ManagedTokenWorker) rotate(ctx context.Context, id pgtype.UUID, cfg installConfig) (installConfig, bool, error) {
	refresh, err := decryptToken(cfg.RefreshTokenEncrypted, w.box.Open)
	if err != nil || refresh == "" {
		return cfg, false, errors.New("managed Slack refresh credential is unavailable")
	}
	form := url.Values{"grant_type": {"refresh_token"}, "refresh_token": {refresh}}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, w.oauth.tokenURL, strings.NewReader(form.Encode()))
	if err != nil {
		return cfg, false, err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.SetBasicAuth(w.oauth.clientID, w.oauth.clientSecret)
	body, err := w.request(ctx, req)
	if err != nil {
		return cfg, false, err
	}
	var response struct {
		OK           bool   `json:"ok"`
		AccessToken  string `json:"access_token"`
		RefreshToken string `json:"refresh_token"`
		ExpiresIn    int64  `json:"expires_in"`
	}
	if json.Unmarshal(body, &response) != nil || !response.OK || response.AccessToken == "" || response.RefreshToken == "" {
		return cfg, false, errors.New("Slack rejected the token refresh")
	}
	expires, err := managedTokenExpiry(response.ExpiresIn, w.now())
	if err != nil {
		return cfg, false, err
	}
	accessSealed, err := w.box.Seal([]byte(response.AccessToken))
	if err != nil {
		return cfg, false, err
	}
	refreshSealed, err := w.box.Seal([]byte(response.RefreshToken))
	if err != nil {
		return cfg, false, err
	}
	next := cfg
	next.BotTokenEncrypted = base64.StdEncoding.EncodeToString(accessSealed)
	next.RefreshTokenEncrypted = base64.StdEncoding.EncodeToString(refreshSealed)
	next.TokenExpiresAt = &expires
	changed, err := w.q.RotateManagedSlackTokens(ctx, db.RotateManagedSlackTokensParams{
		InstallationID: id, PreviousRefreshToken: cfg.RefreshTokenEncrypted,
		BotTokenEncrypted: next.BotTokenEncrypted, RefreshTokenEncrypted: next.RefreshTokenEncrypted,
		TokenExpiresAt: pgtype.Timestamptz{Time: expires, Valid: true},
	})
	return next, changed == 1, err
}

func (w *ManagedTokenWorker) request(ctx context.Context, req *http.Request) ([]byte, error) {
	requestCtx, cancel := context.WithTimeout(ctx, managedHTTPTimeout)
	defer cancel()
	response, err := w.oauth.httpClient.Do(req.WithContext(requestCtx))
	if err != nil {
		return nil, errors.New("managed Slack provider request failed")
	}
	defer response.Body.Close()
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return nil, errors.New("managed Slack provider returned an unsuccessful status")
	}
	return io.ReadAll(io.LimitReader(response.Body, 1<<20))
}

func (w *ManagedTokenWorker) recordHealth(ctx context.Context, id pgtype.UUID, cfg installConfig, state, code, summary string) {
	if ctx.Err() != nil {
		return
	}
	if _, err := w.q.ObserveManagedSlackRuntime(ctx, db.ObserveManagedSlackRuntimeParams{
		InstallationID: id, ExpectedBotToken: cfg.BotTokenEncrypted,
		State: state, ErrorCode: code, ErrorSummary: summary,
	}); err != nil {
		w.logger.WarnContext(ctx, "managed Slack health observation failed", "installation_id", id)
	}
}
