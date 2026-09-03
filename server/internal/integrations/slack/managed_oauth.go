package slack

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// This file is the managed (hosted) Slack OAuth backend — the Go counterpart
// of the Rust slice's slack_managed begin/callback path. Self-hosted
// deployments stay on the BYO Socket Mode path (byo_install.go); this service
// owns the account-level OAuth state for the hosted flow: minting single-use,
// ten-minute, hash-stored state tokens and consuming them exactly once on the
// public callback. Only the state HASH reaches the database, so a table read
// never yields a live callback token.
//
// The live token exchange (ExchangeCode, oauth.v2.access) performs no
// credential handling of its own: the managed client id/secret arrive via
// config, and the HTTP client is injected so tests point it at httptest.

// ManagedOAuthStateTTL bounds one install authorization, matching the Rust
// OAUTH_STATE_TTL. The sweeper (PurgeExpiredSlackOAuthStates, called at the
// head of every BeginInstall) reclaims abandoned rows; workspace teardown
// drops the rest outright.
const ManagedOAuthStateTTL = 10 * time.Minute

// ManagedSlackBotScopes is the scope set requested for the hosted Slack app,
// mirroring the Rust SLACK_BOT_SCOPES.
const ManagedSlackBotScopes = "app_mentions:read,channels:history,chat:write,commands,files:read,groups:history,im:history,mpim:history,reactions:write,users:read"

// DefaultSlackOAuthTokenURL is Slack's token endpoint. Overridden in tests.
const DefaultSlackOAuthTokenURL = "https://slack.com/api/oauth.v2.access"

// ErrInvalidOAuthState surfaces unknown, expired, or already-consumed state.
// All three render the same "restart the install" answer: distinguishing them
// would let a stranger probe which states are live.
var ErrInvalidOAuthState = errors.New("slack: invalid or expired OAuth state")

// ErrInvalidRedirectURL surfaces a post-install redirect_url that is not a
// usable absolute URL. The begin handler maps it to 400 so the installer can
// fix the URL; every other BeginInstall failure is infrastructure (500).
var ErrInvalidRedirectURL = errors.New("slack: invalid redirect_url")

// managedOAuthQueries is the slice of generated queries the service needs.
// *db.Queries satisfies it; tests supply an in-memory fake.
type managedOAuthQueries interface {
	CreateSlackOAuthState(ctx context.Context, arg db.CreateSlackOAuthStateParams) (db.SlackOauthState, error)
	ConsumeSlackOAuthState(ctx context.Context, stateHash []byte) (db.SlackOauthState, error)
	PurgeExpiredSlackOAuthStates(ctx context.Context, expiresAt pgtype.Timestamptz) error
}

// ManagedOAuthConfig configures the hosted install backend. Queries is
// required; ClientID/ClientSecret enable the live code exchange (empty keeps
// state issuance working while refusing the exchange with a clear error, so a
// deployment without hosted credentials fails loudly instead of half-working).
type ManagedOAuthConfig struct {
	Queries      *db.Queries
	ClientID     string
	ClientSecret string
	HTTPClient   *http.Client
	TokenURL     string
	Logger       *slog.Logger
}

// ManagedOAuthService owns hosted Slack install authorizations.
type ManagedOAuthService struct {
	q            managedOAuthQueries
	clientID     string
	clientSecret string
	httpClient   *http.Client
	tokenURL     string
	logger       *slog.Logger
	now          func() time.Time
}

// NewManagedOAuthService builds the service. A nil clock uses time.Now; tests
// inject a pinned clock for expiry assertions.
func NewManagedOAuthService(cfg ManagedOAuthConfig) (*ManagedOAuthService, error) {
	if cfg.Queries == nil {
		return nil, errors.New("slack: ManagedOAuthService requires queries")
	}
	httpClient := cfg.HTTPClient
	if httpClient == nil {
		httpClient = http.DefaultClient
	}
	tokenURL := cfg.TokenURL
	if tokenURL == "" {
		tokenURL = DefaultSlackOAuthTokenURL
	}
	logger := cfg.Logger
	if logger == nil {
		logger = slog.Default()
	}
	return &ManagedOAuthService{
		q:            cfg.Queries,
		clientID:     cfg.ClientID,
		clientSecret: cfg.ClientSecret,
		httpClient:   httpClient,
		tokenURL:     tokenURL,
		logger:       logger,
		now:          time.Now,
	}, nil
}

// BeginInstall starts one hosted authorization: it purges expired states,
// validates the post-install redirect, mints a fresh single-use state token,
// and persists only its hash. It returns the RAW state for the authorize URL;
// the raw value is never stored.
func (s *ManagedOAuthService) BeginInstall(ctx context.Context, workspaceID, installerID pgtype.UUID, redirectURL string) (state string, expiresAt time.Time, err error) {
	parsed, perr := url.ParseRequestURI(redirectURL)
	if perr != nil || (parsed.Scheme != "https" && parsed.Scheme != "http") || parsed.Host == "" {
		return "", time.Time{}, fmt.Errorf("%w: %q must be an absolute http(s) URL", ErrInvalidRedirectURL, redirectURL)
	}
	now := s.now()
	if err := s.q.PurgeExpiredSlackOAuthStates(ctx, pgtype.Timestamptz{Time: now, Valid: true}); err != nil {
		return "", time.Time{}, fmt.Errorf("purge expired slack oauth states: %w", err)
	}
	raw := make([]byte, 32)
	if _, err := rand.Read(raw); err != nil {
		return "", time.Time{}, fmt.Errorf("mint slack oauth state: %w", err)
	}
	encoded := base64.RawURLEncoding.EncodeToString(raw)
	sum := sha256.Sum256([]byte(encoded))
	expiresAt = now.Add(ManagedOAuthStateTTL)
	if _, err := s.q.CreateSlackOAuthState(ctx, db.CreateSlackOAuthStateParams{
		StateHash:       sum[:],
		WorkspaceID:     workspaceID,
		InstallerUserID: installerID,
		RedirectUrl:     redirectURL,
		ExpiresAt:       pgtype.Timestamptz{Time: expiresAt, Valid: true},
	}); err != nil {
		return "", time.Time{}, fmt.Errorf("record slack oauth state: %w", err)
	}
	return encoded, expiresAt, nil
}

// ClientID reports the configured managed Slack client id ("" when the
// deployment has none). The begin handler needs it to build the authorize URL
// and fails loudly (503) when it is empty, while state issuance keeps working.
func (s *ManagedOAuthService) ClientID() string {
	return s.clientID
}

// ConsumeState claims the authorization bound to raw state exactly once. An
// unknown, expired, or already-consumed state all surface ErrInvalidOAuthState.
func (s *ManagedOAuthService) ConsumeState(ctx context.Context, rawState string) (db.SlackOauthState, error) {
	raw := strings.TrimSpace(rawState)
	if raw == "" {
		return db.SlackOauthState{}, ErrInvalidOAuthState
	}
	sum := sha256.Sum256([]byte(raw))
	row, err := s.q.ConsumeSlackOAuthState(ctx, sum[:])
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return db.SlackOauthState{}, ErrInvalidOAuthState
		}
		return db.SlackOauthState{}, err
	}
	return row, nil
}

// AuthorizeURL builds the Slack authorize URL the installer visits. The state
// is the raw token from BeginInstall; Slack returns it verbatim to the
// callback, where ConsumeState verifies it.
func AuthorizeURL(clientID, redirectURI, state string) string {
	v := url.Values{}
	v.Set("client_id", clientID)
	v.Set("scope", ManagedSlackBotScopes)
	v.Set("redirect_uri", redirectURI)
	v.Set("state", state)
	return "https://slack.com/oauth/v2/authorize?" + v.Encode()
}

// OAuthAccess is the subset of the oauth.v2.access response the install
// callback persists: the bot token plus the tenant identity it belongs to.
type OAuthAccess struct {
	BotToken   string
	AppID      string
	TeamID     string
	BotUserID  string
	AuthedUser string
	// RefreshToken and ExpiresAt are the rotating credentials an app with the
	// refresh grant returns. Zero values mean the app has no refresh grant;
	// BYO installs never carry them. ExpiresAt is derived from expires_in at
	// exchange time (the service's clock, so tests can pin it).
	RefreshToken string
	ExpiresAt    time.Time
}

// ExchangeCode trades a callback code for a bot token via oauth.v2.access.
// The HTTP client is injected (tests use httptest); no workspace state is
// touched here — the caller consumes state first, then persists the
// installation through InstallService.
func (s *ManagedOAuthService) ExchangeCode(ctx context.Context, code, redirectURI string) (OAuthAccess, error) {
	if s.clientID == "" || s.clientSecret == "" {
		return OAuthAccess{}, errors.New("slack: managed OAuth is not configured (missing client credentials)")
	}
	form := url.Values{}
	form.Set("client_id", s.clientID)
	form.Set("client_secret", s.clientSecret)
	form.Set("code", code)
	form.Set("redirect_uri", redirectURI)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, s.tokenURL, strings.NewReader(form.Encode()))
	if err != nil {
		return OAuthAccess{}, fmt.Errorf("slack oauth exchange: %w", err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := s.httpClient.Do(req)
	if err != nil {
		return OAuthAccess{}, fmt.Errorf("slack oauth exchange: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return OAuthAccess{}, errors.New("slack oauth exchange returned an unsuccessful status")
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return OAuthAccess{}, fmt.Errorf("slack oauth exchange: %w", err)
	}
	var out struct {
		OK           bool   `json:"ok"`
		Error        string `json:"error"`
		AccessToken  string `json:"access_token"`
		RefreshToken string `json:"refresh_token"`
		ExpiresIn    int64  `json:"expires_in"`
		AppID        string `json:"app_id"`
		BotUserID    string `json:"bot_user_id"`
		Team         struct {
			ID string `json:"id"`
		} `json:"team"`
		AuthedUser struct {
			ID string `json:"id"`
		} `json:"authed_user"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		return OAuthAccess{}, fmt.Errorf("slack oauth exchange: %w", err)
	}
	if !out.OK || out.AccessToken == "" || out.Team.ID == "" {
		return OAuthAccess{}, fmt.Errorf("slack oauth exchange refused: %s", out.Error)
	}
	var expiresAt time.Time
	if out.RefreshToken != "" || out.ExpiresIn != 0 {
		if out.RefreshToken == "" {
			return OAuthAccess{}, errors.New("slack oauth exchange omitted the refresh credential")
		}
		expires, err := managedTokenExpiry(out.ExpiresIn, s.now())
		if err != nil {
			return OAuthAccess{}, err
		}
		expiresAt = expires
	}
	return OAuthAccess{
		BotToken:     out.AccessToken,
		AppID:        out.AppID,
		TeamID:       out.Team.ID,
		BotUserID:    out.BotUserID,
		AuthedUser:   out.AuthedUser.ID,
		RefreshToken: out.RefreshToken,
		ExpiresAt:    expiresAt,
	}, nil
}

// Both initial authorization and refresh must reject an invalid lifetime
// before duration conversion can overflow or an unusable token is persisted.
func managedTokenExpiry(seconds int64, now time.Time) (time.Time, error) {
	if seconds <= 0 || seconds > int64((1<<63-1)/time.Second) {
		return time.Time{}, errors.New("slack returned an invalid access-token lifetime")
	}
	return now.Add(time.Duration(seconds) * time.Second).UTC(), nil
}
