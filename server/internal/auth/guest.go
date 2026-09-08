package auth

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	db "github.com/orvilo-ai/orvilo/server/pkg/db/generated"
)

const GuestTokenPrefix = "ovg_"
const GuestSessionActive = "active"

var ErrInvalidGuestToken = errors.New("invalid guest token")

type GuestIdentityQueries interface {
	GetGuestSessionByTokenHash(context.Context, string) (db.GuestSession, error)
	GetUser(context.Context, pgtype.UUID) (db.User, error)
}

func ValidGuestToken(raw string) bool {
	if len(raw) != len(GuestTokenPrefix)+40 || !strings.HasPrefix(raw, GuestTokenPrefix) {
		return false
	}
	_, err := hex.DecodeString(raw[len(GuestTokenPrefix):])
	return err == nil
}

// ResolveGuestUser is shared by HTTP and realtime authentication. Never cache
// this lookup: a claimed/revoked session must stop authenticating immediately.
// Workspace membership is checked by each transport after resolving identity.
func ResolveGuestUser(ctx context.Context, queries GuestIdentityQueries, raw string) (db.User, error) {
	if !ValidGuestToken(raw) {
		return db.User{}, ErrInvalidGuestToken
	}
	if queries == nil {
		return db.User{}, errors.New("guest identity store unavailable")
	}
	session, err := queries.GetGuestSessionByTokenHash(ctx, HashToken(raw))
	if errors.Is(err, pgx.ErrNoRows) {
		return db.User{}, ErrInvalidGuestToken
	}
	if err != nil {
		return db.User{}, fmt.Errorf("guest session lookup: %w", err)
	}
	if session.Status != GuestSessionActive || !session.ID.Valid || !session.UserID.Valid {
		return db.User{}, ErrInvalidGuestToken
	}
	user, err := queries.GetUser(ctx, session.UserID)
	if errors.Is(err, pgx.ErrNoRows) {
		return db.User{}, ErrInvalidGuestToken
	}
	if err != nil {
		return db.User{}, fmt.Errorf("guest user lookup: %w", err)
	}
	if !user.IsGuest || !user.ID.Valid || user.ID != session.UserID {
		return db.User{}, ErrInvalidGuestToken
	}
	return user, nil
}
