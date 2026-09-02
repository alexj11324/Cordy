package weixin

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/redis/go-redis/v9"
)

const (
	installSessionTTL        = 5 * time.Minute
	InstallSessionTTLSeconds = int(installSessionTTL / time.Second)
	installSessionCap        = 1024
	installSessionKey        = "patchbay:{weixin_install_session}:"
	redisSessionTimeout      = 250 * time.Millisecond
)

// InstallSession is the ephemeral QR handshake state. Bot tokens are kept
// only until Status finalizes the installation and are never returned by an
// HTTP handler or serialized in a public response.
type InstallSession struct {
	ID              string    `json:"id"`
	WorkspaceID     string    `json:"workspace_id"`
	AgentID         string    `json:"agent_id"`
	InitiatorID     string    `json:"initiator_id"`
	QRCode          string    `json:"qrcode"`
	QRCodeImageData string    `json:"qrcode_img_content,omitempty"`
	ExpiresAt       time.Time `json:"expires_at"`
	Status          string    `json:"status,omitempty"`
	BotID           string    `json:"bot_id,omitempty"`
	ILinkUserID     string    `json:"ilink_user_id,omitempty"`
	BaseURL         string    `json:"base_url,omitempty"`
	BotToken        string    `json:"bot_token,omitempty"`
	InstallationID  string    `json:"installation_id,omitempty"`
}

type SessionStore interface {
	Put(context.Context, InstallSession) error
	Get(context.Context, string) (InstallSession, error)
	Delete(context.Context, string) error
}

var (
	storeMu      sync.RWMutex
	defaultStore SessionStore = NewMemorySessionStore()
)

// ConfigureSessionStore enables Redis-backed install sessions when Redis is
// available. A memory fallback remains in place for local/single-process
// deployments and for a transient Redis outage; the cap and TTL are the same
// in both stores.
func ConfigureSessionStore(client redis.UniversalClient) {
	storeMu.Lock()
	defer storeMu.Unlock()
	if client == nil {
		defaultStore = NewMemorySessionStore()
		return
	}
	defaultStore = &fallbackSessionStore{primary: &redisSessionStore{client: client}, fallback: NewMemorySessionStore()}
}

func DefaultInstallSessionStore() SessionStore {
	storeMu.RLock()
	defer storeMu.RUnlock()
	return defaultStore
}

type memorySessionStore struct {
	mu      sync.Mutex
	entries map[string]InstallSession
}

func NewMemorySessionStore() SessionStore {
	return &memorySessionStore{entries: make(map[string]InstallSession)}
}

func (s *memorySessionStore) Put(_ context.Context, session InstallSession) error {
	session.ID = strings.TrimSpace(session.ID)
	if session.ID == "" || session.ExpiresAt.IsZero() {
		return errors.New("weixin: invalid install session")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.pruneLocked(time.Now())
	if _, exists := s.entries[session.ID]; !exists && len(s.entries) >= installSessionCap {
		return errors.New("weixin: install session capacity reached")
	}
	s.entries[session.ID] = session
	return nil
}

func (s *memorySessionStore) Get(_ context.Context, id string) (InstallSession, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.pruneLocked(time.Now())
	session, ok := s.entries[strings.TrimSpace(id)]
	if !ok {
		return InstallSession{}, ErrInstallSessionNotFound
	}
	return session, nil
}

func (s *memorySessionStore) Delete(_ context.Context, id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.entries, strings.TrimSpace(id))
	return nil
}

func (s *memorySessionStore) pruneLocked(now time.Time) {
	for id, session := range s.entries {
		if !session.ExpiresAt.After(now) {
			delete(s.entries, id)
		}
	}
}

type redisSessionStore struct {
	client redis.UniversalClient
}

func (s *redisSessionStore) Put(ctx context.Context, session InstallSession) error {
	session.ID = strings.TrimSpace(session.ID)
	if session.ID == "" || session.ExpiresAt.IsZero() {
		return errors.New("weixin: invalid install session")
	}
	payload, err := json.Marshal(session)
	if err != nil {
		return err
	}
	remaining := time.Until(session.ExpiresAt)
	if remaining <= 0 {
		return ErrInstallSessionExpired
	}
	operationCtx, cancel := context.WithTimeout(ctx, redisSessionTimeout)
	defer cancel()
	return s.client.Set(operationCtx, installSessionKey+session.ID, payload, remaining).Err()
}

func (s *redisSessionStore) Get(ctx context.Context, id string) (InstallSession, error) {
	operationCtx, cancel := context.WithTimeout(ctx, redisSessionTimeout)
	defer cancel()
	payload, err := s.client.Get(operationCtx, installSessionKey+strings.TrimSpace(id)).Bytes()
	if errors.Is(err, redis.Nil) {
		return InstallSession{}, ErrInstallSessionNotFound
	}
	if err != nil {
		return InstallSession{}, err
	}
	var session InstallSession
	if err := json.Unmarshal(payload, &session); err != nil {
		return InstallSession{}, fmt.Errorf("decode redis weixin install session: %w", err)
	}
	if !session.ExpiresAt.After(time.Now()) {
		return InstallSession{}, ErrInstallSessionExpired
	}
	return session, nil
}

func (s *redisSessionStore) Delete(ctx context.Context, id string) error {
	operationCtx, cancel := context.WithTimeout(ctx, redisSessionTimeout)
	defer cancel()
	return s.client.Del(operationCtx, installSessionKey+strings.TrimSpace(id)).Err()
}

type fallbackSessionStore struct {
	primary  SessionStore
	fallback SessionStore
}

func (s *fallbackSessionStore) Put(ctx context.Context, session InstallSession) error {
	if err := s.primary.Put(ctx, session); err == nil {
		return nil
	}
	return s.fallback.Put(ctx, session)
}

func (s *fallbackSessionStore) Get(ctx context.Context, id string) (InstallSession, error) {
	session, err := s.primary.Get(ctx, id)
	if err == nil {
		return session, err
	}
	fallbackSession, fallbackErr := s.fallback.Get(ctx, id)
	if fallbackErr == nil {
		return fallbackSession, nil
	}
	if errors.Is(err, ErrInstallSessionNotFound) || errors.Is(err, ErrInstallSessionExpired) {
		return session, err
	}
	return InstallSession{}, fmt.Errorf("weixin: primary install session store failed: %w", err)
}

func (s *fallbackSessionStore) Delete(ctx context.Context, id string) error {
	primaryErr := s.primary.Delete(ctx, id)
	fallbackErr := s.fallback.Delete(ctx, id)
	if primaryErr != nil {
		return primaryErr
	}
	return fallbackErr
}
