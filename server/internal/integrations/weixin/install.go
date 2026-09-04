package weixin

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgtype"

	"github.com/patchbay-ai/patchbay/server/internal/hostedcapacity"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

var (
	ErrInstallationNotFound        = errors.New("weixin: installation not found")
	ErrInstallSessionNotFound      = errors.New("weixin: install session not found")
	ErrInstallSessionExpired       = errors.New("weixin: install session expired")
	ErrInstallSessionForbidden     = errors.New("weixin: install session is not yours")
	ErrInstallAuthorizationChanged = errors.New("weixin: authorization changed during install")
	ErrBotOwnedByAnotherWorkspace  = errors.New("weixin: this account is already connected to another workspace")
	ErrBotOwnedBySameWorkspace     = errors.New("weixin: this account is already connected in this workspace")
	ErrBotOwnedByArchivedAgent     = errors.New("weixin: this account is connected to an archived agent")
	ErrUnsafeProviderURL           = errors.New("weixin: provider returned an unsafe redirect host")
	ErrConfirmationIncomplete      = errors.New("weixin: confirmation was incomplete")
)

const (
	InstallStatusPending          = "pending"
	InstallStatusScanned          = "scanned"
	InstallStatusNeedVerifyCode   = "need_verify_code"
	InstallStatusAlreadyConnected = "already_connected"
	InstallStatusExpired          = "expired"
	InstallStatusSuccess          = "success"
)

type BeginParams struct {
	WorkspaceID pgtype.UUID
	AgentID     pgtype.UUID
	InitiatorID pgtype.UUID
}

type BeginResult struct {
	SessionID           string
	QRCode              string
	QRCodeImageData     string
	ExpiresAt           time.Time
	PollIntervalSeconds int
}

type StatusResult struct {
	Status         string
	InstallationID pgtype.UUID
	// Created is true only for the poll that finalized a new installation. It
	// keeps the HTTP layer from rebroadcasting the created lifecycle event when
	// the client polls an already-completed install session.
	Created bool
}

type InstallationService struct {
	q          *db.Queries
	tx         bindingTxStarter
	box        *secretbox.Box
	sessions   SessionStore
	httpClient *http.Client
	logger     *slog.Logger
	now        func() time.Time
	newClient  func(string, string, *http.Client) *Client
	// capacity, when set, re-resolves the hosted installation cap at QR
	// finalize and refuses over-cap installs inside the finalize transaction.
	capacity *hostedcapacity.Limiter
	mu       sync.Mutex
}

func NewInstallationService(q *db.Queries, tx bindingTxStarter, box *secretbox.Box, sessions SessionStore, logger *slog.Logger) (*InstallationService, error) {
	if q == nil || tx == nil || box == nil {
		return nil, errors.New("weixin: installation service requires queries, tx starter and secretbox")
	}
	if sessions == nil {
		sessions = DefaultInstallSessionStore()
	}
	if logger == nil {
		logger = slog.Default()
	}
	return &InstallationService{
		q: q, tx: tx, box: box, sessions: sessions,
		httpClient: &http.Client{Timeout: 40 * time.Second}, logger: logger,
		now: time.Now, newClient: NewClient,
	}, nil
}

// SetHostedCapacityLimiter wires the managed-deployment installation cap. The
// QR finalize re-resolves the limit through it (never reusing the value from
// Begin — a subscription can change mid-scan) and refuses over-cap installs
// inside the finalize transaction. nil (self-hosted) finalizes uncapped.
func (s *InstallationService) SetHostedCapacityLimiter(limiter *hostedcapacity.Limiter) {
	s.capacity = limiter
}

func (s *InstallationService) Begin(ctx context.Context, p BeginParams) (BeginResult, error) {
	if s == nil || s.q == nil || !p.WorkspaceID.Valid || !p.InitiatorID.Valid {
		return BeginResult{}, errors.New("weixin: invalid install parameters")
	}
	rows, err := s.q.ListChannelInstallationsByWorkspace(ctx, db.ListChannelInstallationsByWorkspaceParams{
		WorkspaceID: p.WorkspaceID, ChannelType: string(TypeWeixin),
	})
	if err != nil {
		return BeginResult{}, fmt.Errorf("weixin: list existing installations: %w", err)
	}
	localTokens := make([]string, 0, 1)
	for _, row := range rows {
		if row.AgentID != p.AgentID {
			continue
		}
		credentials, decodeErr := DecodeCredentials(row.Config, s.box.Open)
		if decodeErr == nil && credentials.BotToken != "" {
			localTokens = append(localTokens, credentials.BotToken)
		}
	}
	client := s.client(DefaultBaseURL, "")
	qr, err := client.RequestQRCode(ctx, localTokens)
	if err != nil {
		return BeginResult{}, fmt.Errorf("weixin: request QR code: %w", err)
	}
	now := s.clock()
	session := InstallSession{
		ID: uuid.NewString(), WorkspaceID: util.UUIDToString(p.WorkspaceID), AgentID: util.UUIDToString(p.AgentID),
		InitiatorID: util.UUIDToString(p.InitiatorID), QRCode: qr.QRCode, QRCodeImageData: qr.QRCodeImageData,
		ExpiresAt: now.Add(installSessionTTL), Status: InstallStatusPending, BaseURL: DefaultBaseURL,
	}
	if err := s.sessions.Put(ctx, session); err != nil {
		return BeginResult{}, fmt.Errorf("weixin: store install session: %w", err)
	}
	return BeginResult{
		SessionID: session.ID, QRCode: session.QRCode, QRCodeImageData: session.QRCodeImageData,
		ExpiresAt: session.ExpiresAt, PollIntervalSeconds: 2,
	}, nil
}

func (s *InstallationService) Status(ctx context.Context, sessionID string, workspaceID, actorID pgtype.UUID, verifyCode string) (StatusResult, error) {
	if s == nil || s.sessions == nil || strings.TrimSpace(sessionID) == "" || !workspaceID.Valid || !actorID.Valid {
		return StatusResult{}, ErrInstallSessionNotFound
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	session, err := s.sessions.Get(ctx, sessionID)
	if err != nil {
		if errors.Is(err, ErrInstallSessionExpired) || errors.Is(err, ErrInstallSessionNotFound) {
			return StatusResult{}, ErrInstallSessionNotFound
		}
		return StatusResult{}, fmt.Errorf("weixin: load install session: %w", err)
	}
	if session.WorkspaceID != util.UUIDToString(workspaceID) || session.InitiatorID != util.UUIDToString(actorID) {
		return StatusResult{}, ErrInstallSessionForbidden
	}
	if session.InstallationID != "" {
		installationID, parseErr := util.ParseUUID(session.InstallationID)
		if parseErr != nil || !installationID.Valid {
			return StatusResult{}, errors.New("weixin: stored install session has invalid installation id")
		}
		return StatusResult{Status: InstallStatusSuccess, InstallationID: installationID}, nil
	}
	if !session.ExpiresAt.After(s.clock()) {
		session.Status = InstallStatusExpired
		_ = s.sessions.Put(ctx, session)
		return StatusResult{Status: InstallStatusExpired}, nil
	}
	baseURL, err := ValidateProviderBaseURL(session.BaseURL)
	if err != nil {
		return StatusResult{}, ErrUnsafeProviderURL
	}
	status, err := s.client(baseURL, "").QRStatus(ctx, session.QRCode, verifyCode)
	if err != nil {
		// QR status is polled by the UI. A transient provider/network failure
		// must not turn a live session into a terminal error.
		return StatusResult{Status: InstallStatusPending}, nil
	}
	switch strings.ToLower(strings.TrimSpace(status.Status)) {
	case "wait":
		return StatusResult{Status: InstallStatusPending}, nil
	case "scaned", "scanned":
		return StatusResult{Status: InstallStatusScanned}, nil
	case "need_verifycode":
		return StatusResult{Status: InstallStatusNeedVerifyCode}, nil
	case "verify_code_blocked", "expired":
		session.Status = InstallStatusExpired
		_ = s.sessions.Put(ctx, session)
		return StatusResult{Status: InstallStatusExpired}, nil
	case "scaned_but_redirect", "scanned_but_redirect":
		redirect, validateErr := ValidateProviderBaseURL(status.RedirectHost)
		if validateErr != nil {
			return StatusResult{}, ErrUnsafeProviderURL
		}
		session.BaseURL = redirect
		if err := s.sessions.Put(ctx, session); err != nil {
			return StatusResult{}, fmt.Errorf("weixin: persist redirect host: %w", err)
		}
		return StatusResult{Status: InstallStatusScanned}, nil
	case "binded_redirect":
		return StatusResult{Status: InstallStatusAlreadyConnected}, nil
	case "confirmed":
		// continue to the credential finalization below
	default:
		return StatusResult{Status: InstallStatusPending}, nil
	}
	if strings.TrimSpace(status.BotToken) == "" || strings.TrimSpace(status.ILinkBotID) == "" || strings.TrimSpace(status.ILinkUserID) == "" {
		return StatusResult{}, ErrConfirmationIncomplete
	}
	sealed, err := s.box.Seal([]byte(status.BotToken))
	if err != nil {
		return StatusResult{}, fmt.Errorf("weixin: encrypt bot token: %w", err)
	}
	providerBaseURL := status.BaseURL
	if strings.TrimSpace(providerBaseURL) == "" {
		providerBaseURL = session.BaseURL
	}
	providerBaseURL, err = ValidateProviderBaseURL(providerBaseURL)
	if err != nil {
		providerBaseURL = DefaultBaseURL
	}
	config, err := encodeInstallConfig(status.ILinkBotID, status.ILinkUserID, providerBaseURL, base64.StdEncoding.EncodeToString(sealed))
	if err != nil {
		return StatusResult{}, err
	}
	workspaceUUID, err := util.ParseUUID(session.WorkspaceID)
	if err != nil {
		return StatusResult{}, ErrInstallSessionNotFound
	}
	var agentUUID pgtype.UUID
	if session.AgentID != "" {
		agentUUID, err = util.ParseUUID(session.AgentID)
		if err != nil {
			return StatusResult{}, ErrInstallSessionNotFound
		}
	}
	initiatorUUID, err := util.ParseUUID(session.InitiatorID)
	if err != nil {
		return StatusResult{}, ErrInstallSessionNotFound
	}
	row, err := s.finalize(ctx, workspaceUUID, agentUUID, initiatorUUID, status.ILinkBotID, status.ILinkUserID, config)
	if err != nil {
		return StatusResult{}, err
	}
	session.Status = InstallStatusSuccess
	session.InstallationID = util.UUIDToString(row.ID)
	session.BotID, session.ILinkUserID, session.BotToken = "", "", ""
	if err := s.sessions.Put(ctx, session); err != nil {
		return StatusResult{}, fmt.Errorf("weixin: store completed install session: %w", err)
	}
	return StatusResult{Status: InstallStatusSuccess, InstallationID: row.ID, Created: true}, nil
}

func (s *InstallationService) finalize(ctx context.Context, workspaceID, agentID, installerID pgtype.UUID, botID, userID string, config []byte) (db.ChannelInstallation, error) {
	if !workspaceID.Valid || !installerID.Valid {
		return db.ChannelInstallation{}, errors.New("weixin: invalid installation scope")
	}
	if err := validateInstallConfig(botID, userID, config); err != nil {
		return db.ChannelInstallation{}, err
	}
	// Re-resolve the hosted cap now, at the finalize the user waited for —
	// never reusing a value captured at Begin, where a subscription could
	// have changed mid-scan.
	var limit *int64
	if s.capacity != nil {
		resolved, err := s.capacity.InstallationLimit(ctx, workspaceID)
		if err != nil {
			return db.ChannelInstallation{}, err
		}
		limit = resolved
	}
	tx, err := s.tx.Begin(ctx)
	if err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("weixin: begin installation tx: %w", err)
	}
	defer func() { _ = tx.Rollback(ctx) }()
	qtx := s.q.WithTx(tx)
	// Hosted-capacity admission first, under the workspace row lock — the
	// same lock order every install path and the reconciler use, so a
	// capacity change can never interleave with a finalize.
	if err := hostedcapacity.AdmitInstall(ctx, qtx, workspaceID, string(TypeWeixin), agentID, limit); err != nil {
		return db.ChannelInstallation{}, err
	}
	if agentID.Valid {
		if err := lockInstallationAgentSlot(ctx, tx, workspaceID, agentID); err != nil {
			return db.ChannelInstallation{}, err
		}
	}
	if err := qtx.LockChannelInstallationAppIDSlot(ctx, db.LockChannelInstallationAppIDSlotParams{
		ChannelType: string(TypeWeixin), AppID: botID,
	}); err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("weixin: lock installation app slot: %w", err)
	}
	member, err := qtx.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{UserID: installerID, WorkspaceID: workspaceID})
	if err != nil || !member.ID.Valid {
		return db.ChannelInstallation{}, ErrInstallAuthorizationChanged
	}
	if agentID.Valid {
		agent, err := qtx.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: agentID, WorkspaceID: workspaceID})
		if err != nil || !agent.ID.Valid || agent.ArchivedAt.Valid {
			return db.ChannelInstallation{}, ErrInstallAuthorizationChanged
		}
		if member.Role != "owner" && member.Role != "admin" && agent.OwnerID != installerID {
			return db.ChannelInstallation{}, ErrInstallAuthorizationChanged
		}
	} else if member.Role != "owner" && member.Role != "admin" {
		return db.ChannelInstallation{}, ErrInstallAuthorizationChanged
	}
	if reclaimed, reclaimErr := qtx.ReclaimDeadChannelInstallationByAppID(ctx, db.ReclaimDeadChannelInstallationByAppIDParams{
		ChannelType: string(TypeWeixin), AppID: botID, WorkspaceID: workspaceID, AgentID: agentID,
	}); reclaimErr != nil && !errors.Is(reclaimErr, pgx.ErrNoRows) {
		return db.ChannelInstallation{}, fmt.Errorf("weixin: reclaim dead installation: %w", reclaimErr)
	} else if reclaimErr == nil {
		if err := deleteReceiveCursor(ctx, tx, reclaimed); err != nil {
			return db.ChannelInstallation{}, err
		}
	}
	rows, err := qtx.ListChannelInstallationsByWorkspace(ctx, db.ListChannelInstallationsByWorkspaceParams{
		WorkspaceID: workspaceID, ChannelType: string(TypeWeixin),
	})
	if err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("weixin: list agent installations: %w", err)
	}
	if agentID.Valid {
		for _, current := range rows {
			if current.AgentID == agentID && DecodePublicConfig(current.Config).BotID != botID {
				if err := deleteInstallationForReplacement(ctx, tx, current.ID); err != nil {
					return db.ChannelInstallation{}, err
				}
			}
		}
	}
	var row db.ChannelInstallation
	if agentID.Valid {
		row, err = qtx.UpsertChannelInstallation(ctx, db.UpsertChannelInstallationParams{
			WorkspaceID: workspaceID, AgentID: agentID, ChannelType: string(TypeWeixin), Config: config, InstallerUserID: installerID,
		})
	} else {
		row, err = qtx.UpsertChannelInstallationHub(ctx, db.UpsertChannelInstallationHubParams{
			WorkspaceID: workspaceID, ChannelType: string(TypeWeixin), Config: config, InstallerUserID: installerID,
		})
	}
	if err != nil {
		var pgErr *pgconn.PgError
		if errors.As(err, &pgErr) && pgErr.Code == "23505" {
			_ = tx.Rollback(ctx)
			return db.ChannelInstallation{}, s.classifyOwner(ctx, workspaceID, botID)
		}
		return db.ChannelInstallation{}, fmt.Errorf("weixin: upsert installation: %w", err)
	}
	if _, err := qtx.CreateChannelUserBinding(ctx, db.CreateChannelUserBindingParams{
		WorkspaceID: workspaceID, PatchbayUserID: installerID, InstallationID: row.ID,
		ChannelType: string(TypeWeixin), ChannelUserID: strings.TrimSpace(userID), Config: []byte(`{}`),
	}); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return db.ChannelInstallation{}, ErrBindingAlreadyAssigned
		}
		return db.ChannelInstallation{}, fmt.Errorf("weixin: bind scanner account: %w", err)
	}
	if err := tx.Commit(ctx); err != nil {
		return db.ChannelInstallation{}, fmt.Errorf("weixin: commit installation: %w", err)
	}
	return row, nil
}

func validateInstallConfig(botID, userID string, config []byte) error {
	botID = strings.TrimSpace(botID)
	userID = strings.TrimSpace(userID)
	if botID == "" || userID == "" {
		return errors.New("weixin: missing installation identity")
	}
	var decoded installConfig
	if err := json.Unmarshal(config, &decoded); err != nil {
		return fmt.Errorf("weixin: decode installation config: %w", err)
	}
	if strings.TrimSpace(decoded.AppID) != botID || strings.TrimSpace(decoded.ILinkUserID) != userID || strings.TrimSpace(decoded.BotTokenEncrypted) == "" {
		return errors.New("weixin: installation config does not match provider identity")
	}
	if _, err := ValidateProviderBaseURL(decoded.BaseURL); err != nil {
		return fmt.Errorf("weixin: invalid installation base url: %w", err)
	}
	return nil
}

func (s *InstallationService) classifyOwner(ctx context.Context, workspaceID pgtype.UUID, botID string) error {
	owner, err := s.q.GetChannelInstallationOwnerByAppID(ctx, db.GetChannelInstallationOwnerByAppIDParams{ChannelType: string(TypeWeixin), AppID: botID})
	if err != nil {
		return ErrBotOwnedBySameWorkspace
	}
	if owner.WorkspaceID != workspaceID {
		return ErrBotOwnedByAnotherWorkspace
	}
	if owner.AgentArchivedAt.Valid {
		return ErrBotOwnedByArchivedAgent
	}
	return ErrBotOwnedBySameWorkspace
}

func (s *InstallationService) ListByWorkspace(ctx context.Context, workspaceID pgtype.UUID) ([]db.ChannelInstallation, error) {
	if s == nil || s.q == nil || !workspaceID.Valid {
		return nil, errors.New("weixin: installation service is not configured")
	}
	return s.q.ListChannelInstallationsByWorkspace(ctx, db.ListChannelInstallationsByWorkspaceParams{
		WorkspaceID: workspaceID, ChannelType: string(TypeWeixin),
	})
}

func (s *InstallationService) GetInWorkspace(ctx context.Context, installationID, workspaceID pgtype.UUID) (db.ChannelInstallation, error) {
	if s == nil || s.q == nil || !installationID.Valid || !workspaceID.Valid {
		return db.ChannelInstallation{}, ErrInstallationNotFound
	}
	row, err := s.q.GetChannelInstallationInWorkspace(ctx, db.GetChannelInstallationInWorkspaceParams{
		ID: installationID, WorkspaceID: workspaceID, ChannelType: string(TypeWeixin),
	})
	if errors.Is(err, pgx.ErrNoRows) {
		return db.ChannelInstallation{}, ErrInstallationNotFound
	}
	return row, err
}

func (s *InstallationService) Revoke(ctx context.Context, installationID pgtype.UUID) error {
	if s == nil || s.q == nil || !installationID.Valid {
		return ErrInstallationNotFound
	}
	return s.q.SetChannelInstallationStatus(ctx, db.SetChannelInstallationStatusParams{ID: installationID, Status: "revoked"})
}

func (s *InstallationService) clock() time.Time {
	if s.now != nil {
		return s.now()
	}
	return time.Now()
}

func (s *InstallationService) client(baseURL, token string) *Client {
	factory := s.newClient
	if factory == nil {
		factory = NewClient
	}
	return factory(baseURL, token, s.httpClient)
}
