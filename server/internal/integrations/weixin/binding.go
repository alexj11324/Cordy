package weixin

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const BindingTokenTTL = 15 * time.Minute

var (
	ErrBindingTokenInvalid       = errors.New("weixin: binding token invalid or expired")
	ErrBindingAlreadyAssigned    = errors.New("weixin: user id is already bound to a different user")
	ErrBindingNotWorkspaceMember = errors.New("weixin: redeemer is not a workspace member")
)

type BindingToken struct {
	Raw       string
	ExpiresAt time.Time
}

type RedeemedBindingToken struct {
	WorkspaceID    pgtype.UUID
	InstallationID pgtype.UUID
	WeixinUserID   string
}

type bindingTxStarter interface {
	Begin(context.Context) (pgx.Tx, error)
}

type BindingTokenService struct {
	q   *db.Queries
	tx  bindingTxStarter
	now func() time.Time
}

func NewBindingTokenService(q *db.Queries, tx bindingTxStarter) *BindingTokenService {
	return &BindingTokenService{q: q, tx: tx, now: time.Now}
}

func (s *BindingTokenService) Mint(ctx context.Context, workspaceID, installationID pgtype.UUID, weixinUserID string) (BindingToken, error) {
	if s == nil || s.q == nil || !workspaceID.Valid || !installationID.Valid || strings.TrimSpace(weixinUserID) == "" {
		return BindingToken{}, errors.New("weixin: binding service is not configured")
	}
	installation, err := s.q.GetChannelInstallation(ctx, db.GetChannelInstallationParams{
		ID: installationID, ChannelType: string(TypeWeixin),
	})
	if err != nil {
		return BindingToken{}, fmt.Errorf("weixin: load installation for binding: %w", err)
	}
	if installation.WorkspaceID != workspaceID {
		return BindingToken{}, errors.New("weixin: installation workspace mismatch")
	}
	raw, err := randomBindingToken(32)
	if err != nil {
		return BindingToken{}, fmt.Errorf("weixin: generate binding token: %w", err)
	}
	now := time.Now
	if s.now != nil {
		now = s.now
	}
	expiresAt := now().Add(BindingTokenTTL)
	if _, err := s.q.CreateChannelBindingToken(ctx, db.CreateChannelBindingTokenParams{
		TokenHash: hashBindingToken(raw), WorkspaceID: workspaceID, InstallationID: installationID,
		ChannelType: string(TypeWeixin), ChannelUserID: strings.TrimSpace(weixinUserID),
		ExpiresAt: pgtype.Timestamptz{Time: expiresAt, Valid: true},
	}); err != nil {
		return BindingToken{}, fmt.Errorf("weixin: persist binding token: %w", err)
	}
	return BindingToken{Raw: raw, ExpiresAt: expiresAt}, nil
}

func (s *BindingTokenService) RedeemAndBind(ctx context.Context, raw string, patchbayUserID pgtype.UUID) (RedeemedBindingToken, error) {
	if s == nil || s.q == nil || s.tx == nil || !patchbayUserID.Valid || strings.TrimSpace(raw) == "" {
		return RedeemedBindingToken{}, ErrBindingTokenInvalid
	}
	tx, err := s.tx.Begin(ctx)
	if err != nil {
		return RedeemedBindingToken{}, fmt.Errorf("weixin: begin binding tx: %w", err)
	}
	defer func() { _ = tx.Rollback(ctx) }()
	qtx := s.q.WithTx(tx)
	row, err := qtx.ConsumeChannelBindingToken(ctx, hashBindingToken(raw))
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return RedeemedBindingToken{}, ErrBindingTokenInvalid
		}
		return RedeemedBindingToken{}, fmt.Errorf("weixin: consume binding token: %w", err)
	}
	if row.ChannelType != string(TypeWeixin) || strings.TrimSpace(row.ChannelUserID) == "" {
		return RedeemedBindingToken{}, ErrBindingTokenInvalid
	}
	installation, err := qtx.GetChannelInstallation(ctx, db.GetChannelInstallationParams{
		ID: row.InstallationID, ChannelType: string(TypeWeixin),
	})
	if err != nil || installation.WorkspaceID != row.WorkspaceID {
		if err != nil && !errors.Is(err, pgx.ErrNoRows) {
			return RedeemedBindingToken{}, fmt.Errorf("weixin: validate binding installation: %w", err)
		}
		return RedeemedBindingToken{}, ErrBindingTokenInvalid
	}
	if _, err := qtx.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{
		UserID: patchbayUserID, WorkspaceID: row.WorkspaceID,
	}); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return RedeemedBindingToken{}, ErrBindingNotWorkspaceMember
		}
		return RedeemedBindingToken{}, fmt.Errorf("weixin: check binding membership: %w", err)
	}
	if _, err := qtx.CreateChannelUserBinding(ctx, db.CreateChannelUserBindingParams{
		WorkspaceID: row.WorkspaceID, PatchbayUserID: patchbayUserID, InstallationID: row.InstallationID,
		ChannelType: string(TypeWeixin), ChannelUserID: row.ChannelUserID, Config: []byte(`{}`),
	}); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return RedeemedBindingToken{}, ErrBindingAlreadyAssigned
		}
		return RedeemedBindingToken{}, fmt.Errorf("weixin: create user binding: %w", err)
	}
	if err := tx.Commit(ctx); err != nil {
		return RedeemedBindingToken{}, fmt.Errorf("weixin: commit binding: %w", err)
	}
	return RedeemedBindingToken{
		WorkspaceID: row.WorkspaceID, InstallationID: row.InstallationID, WeixinUserID: row.ChannelUserID,
	}, nil
}

func randomBindingToken(size int) (string, error) {
	buf := make([]byte, size)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(buf), nil
}

func hashBindingToken(raw string) string {
	sum := sha256.Sum256([]byte(raw))
	return hex.EncodeToString(sum[:])
}
