package handler

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/golang-jwt/jwt/v5"
)

const clerkSessionClockSkew = 5 * time.Second

var (
	errClerkInvalid     = errors.New("invalid Clerk session")
	errClerkUnavailable = errors.New("Clerk unavailable")
)

type ClerkIdentity struct {
	Email     string
	Name      string
	AvatarURL string
}

type ClerkSessionVerifier interface {
	VerifySession(context.Context, string) (ClerkIdentity, error)
	VerifyFreshSession(context.Context, string, time.Time) (ClerkIdentity, error)
}

type clerkAuthClient struct {
	httpClient        *http.Client
	apiBaseURL        string
	secretKey         string
	issuer            string
	authorizedParties map[string]struct{}
	verificationKey   any
}

type clerkClaims struct {
	Sid string `json:"sid"`
	Azp string `json:"azp"`
	Sts string `json:"sts"`
	jwt.RegisteredClaims
}

type clerkSession struct {
	ID        string          `json:"id"`
	UserID    string          `json:"user_id"`
	ClientID  string          `json:"client_id"`
	CreatedAt int64           `json:"created_at"`
	Status    string          `json:"status"`
	Actor     json.RawMessage `json:"actor"`
}

type clerkUser struct {
	Banned                bool                `json:"banned"`
	Locked                bool                `json:"locked"`
	PrimaryEmailAddressID string              `json:"primary_email_address_id"`
	EmailAddresses        []clerkEmailAddress `json:"email_addresses"`
	FirstName             string              `json:"first_name"`
	LastName              string              `json:"last_name"`
	Username              string              `json:"username"`
	ImageURL              string              `json:"image_url"`
}

type clerkEmailAddress struct {
	ID           string `json:"id"`
	EmailAddress string `json:"email_address"`
	Verification *struct {
		Status string `json:"status"`
	} `json:"verification"`
}

func newClerkAuthClient(cfg Config) (ClerkSessionVerifier, error) {
	configured := cfg.ClerkSecretKey != "" || cfg.ClerkJWTKey != "" || cfg.ClerkIssuer != "" || len(cfg.ClerkAuthorizedParties) != 0
	if !configured {
		return nil, nil
	}
	if cfg.ClerkSecretKey == "" || cfg.ClerkJWTKey == "" || cfg.ClerkIssuer == "" || len(cfg.ClerkAuthorizedParties) == 0 {
		return nil, errors.New("CLERK_SECRET_KEY, CLERK_JWT_KEY, CLERK_ISSUER, and CLERK_AUTHORIZED_PARTIES must be configured together")
	}
	key, err := jwt.ParseRSAPublicKeyFromPEM([]byte(strings.ReplaceAll(cfg.ClerkJWTKey, `\n`, "\n")))
	if err != nil {
		return nil, fmt.Errorf("CLERK_JWT_KEY is invalid: %w", err)
	}
	parties := make(map[string]struct{}, len(cfg.ClerkAuthorizedParties))
	for _, raw := range cfg.ClerkAuthorizedParties {
		parsed, err := url.Parse(raw)
		if err != nil || (parsed.Scheme != "https" && parsed.Scheme != "http") || parsed.Host == "" || parsed.User != nil || parsed.Path != "" && parsed.Path != "/" || parsed.RawQuery != "" || parsed.Fragment != "" {
			return nil, errors.New("CLERK_AUTHORIZED_PARTIES entries must be HTTP(S) origins")
		}
		parties[parsed.Scheme+"://"+parsed.Host] = struct{}{}
	}
	return &clerkAuthClient{httpClient: &http.Client{Timeout: 10 * time.Second}, apiBaseURL: "https://api.clerk.com/v1/", secretKey: cfg.ClerkSecretKey, issuer: strings.TrimRight(cfg.ClerkIssuer, "/"), authorizedParties: parties, verificationKey: key}, nil
}

func (c *clerkAuthClient) VerifySession(ctx context.Context, token string) (ClerkIdentity, error) {
	return c.verifySession(ctx, token, nil)
}

func (c *clerkAuthClient) VerifyFreshSession(ctx context.Context, token string, startedAt time.Time) (ClerkIdentity, error) {
	return c.verifySession(ctx, token, &startedAt)
}

func (c *clerkAuthClient) verifySession(ctx context.Context, token string, startedAt *time.Time) (ClerkIdentity, error) {
	claims := &clerkClaims{}
	parsed, err := jwt.ParseWithClaims(token, claims, func(*jwt.Token) (any, error) { return c.verificationKey, nil }, jwt.WithValidMethods([]string{"RS256"}), jwt.WithIssuer(c.issuer), jwt.WithLeeway(clerkSessionClockSkew))
	if err != nil || !parsed.Valid || claims.Subject == "" || claims.Sid == "" || claims.ExpiresAt == nil || claims.NotBefore == nil || claims.Sts == "pending" {
		return ClerkIdentity{}, errClerkInvalid
	}
	if _, ok := c.authorizedParties[strings.TrimRight(claims.Azp, "/")]; !ok {
		return ClerkIdentity{}, errClerkInvalid
	}
	var session clerkSession
	if err := c.get(ctx, "sessions/"+url.PathEscape(claims.Sid), &session); err != nil {
		return ClerkIdentity{}, err
	}
	createdAt := time.UnixMilli(session.CreatedAt)
	if session.ID != claims.Sid || session.UserID != claims.Subject || session.ClientID == "" || session.Status != "active" || len(session.Actor) != 0 && string(session.Actor) != "null" || startedAt != nil && createdAt.Before(startedAt.Add(-clerkSessionClockSkew)) || createdAt.After(time.Now().Add(clerkSessionClockSkew)) {
		return ClerkIdentity{}, errClerkInvalid
	}
	var user clerkUser
	if err := c.get(ctx, "users/"+url.PathEscape(claims.Subject), &user); err != nil {
		return ClerkIdentity{}, err
	}
	if user.Banned || user.Locked {
		return ClerkIdentity{}, errClerkInvalid
	}
	email := ""
	for _, item := range user.EmailAddresses {
		if item.ID == user.PrimaryEmailAddressID && item.Verification != nil && item.Verification.Status == "verified" {
			email = strings.ToLower(strings.TrimSpace(item.EmailAddress))
			break
		}
	}
	if email == "" {
		return ClerkIdentity{}, errClerkInvalid
	}
	name := strings.TrimSpace(strings.TrimSpace(user.FirstName) + " " + strings.TrimSpace(user.LastName))
	if name == "" {
		name = strings.TrimSpace(user.Username)
	}
	if name == "" {
		name = strings.Split(email, "@")[0]
	}
	return ClerkIdentity{Email: email, Name: name, AvatarURL: strings.TrimSpace(user.ImageURL)}, nil
}

func (c *clerkAuthClient) get(ctx context.Context, path string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.apiBaseURL+path, nil)
	if err != nil {
		return errClerkUnavailable
	}
	req.Header.Set("Authorization", "Bearer "+c.secretKey)
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return errClerkUnavailable
	}
	defer resp.Body.Close()
	if resp.StatusCode == http.StatusNotFound {
		return errClerkInvalid
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return errClerkUnavailable
	}
	decoder := json.NewDecoder(io.LimitReader(resp.Body, 64<<10))
	if err := decoder.Decode(out); err != nil {
		return errClerkUnavailable
	}
	return nil
}
