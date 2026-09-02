package handler

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"encoding/pem"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/golang-jwt/jwt/v5"
)

func TestClerkAuthConfigurationFailsClosed(t *testing.T) {
	if verifier, err := newClerkAuthClient(Config{}); err != nil || verifier != nil {
		t.Fatalf("empty config = (%v, %v), want disabled", verifier, err)
	}
	if _, err := newClerkAuthClient(Config{ClerkSecretKey: "sk_test"}); err == nil {
		t.Fatal("partial Clerk configuration accepted")
	}
	if _, err := newClerkAuthClient(Config{ClerkSecretKey: "sk_test", ClerkJWTKey: "not pem", ClerkIssuer: "https://clerk.example", ClerkAuthorizedParties: []string{"https://accounts.aspectlylabs.com"}}); err == nil || !strings.Contains(err.Error(), "CLERK_JWT_KEY") {
		t.Fatalf("invalid key error = %v", err)
	}
}

func TestClerkAuthVerifiesFreshSessionAndPrimaryEmail(t *testing.T) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	publicPEM := pem.EncodeToMemory(&pem.Block{Type: "PUBLIC KEY", Bytes: mustMarshalPublicKey(t, &key.PublicKey)})
	startedAt := time.Now().Add(-time.Second).Truncate(time.Millisecond)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer sk_test" {
			t.Error("missing Clerk API credential")
		}
		switch r.URL.Path {
		case "/sessions/sess_1":
			fmt.Fprintf(w, `{"id":"sess_1","user_id":"user_1","client_id":"client_1","created_at":%d,"status":"active","actor":null}`, startedAt.Add(500*time.Millisecond).UnixMilli())
		case "/users/user_1":
			fmt.Fprint(w, `{"primary_email_address_id":"email_1","email_addresses":[{"id":"email_1","email_address":"User@Example.com","verification":{"status":"verified"}}],"first_name":"Ada","last_name":"Lovelace","image_url":"https://img.example/ada.png"}`)
		default:
			http.NotFound(w, r)
		}
	}))
	defer server.Close()
	verifier, err := newClerkAuthClient(Config{ClerkSecretKey: "sk_test", ClerkJWTKey: string(publicPEM), ClerkIssuer: "https://clerk.example", ClerkAuthorizedParties: []string{"https://accounts.aspectlylabs.com"}})
	if err != nil {
		t.Fatal(err)
	}
	client := verifier.(*clerkAuthClient)
	client.apiBaseURL = server.URL + "/"
	now := time.Now()
	token := jwt.NewWithClaims(jwt.SigningMethodRS256, clerkClaims{Sid: "sess_1", Azp: "https://accounts.aspectlylabs.com", RegisteredClaims: jwt.RegisteredClaims{Subject: "user_1", Issuer: "https://clerk.example", ExpiresAt: jwt.NewNumericDate(now.Add(time.Minute)), NotBefore: jwt.NewNumericDate(now.Add(-time.Minute))}})
	signed, err := token.SignedString(key)
	if err != nil {
		t.Fatal(err)
	}
	identity, err := client.VerifyFreshSession(t.Context(), signed, startedAt)
	if err != nil {
		t.Fatal(err)
	}
	if identity.Email != "user@example.com" || identity.Name != "Ada Lovelace" {
		t.Fatalf("identity = %+v", identity)
	}
}

func mustMarshalPublicKey(t *testing.T, key *rsa.PublicKey) []byte {
	t.Helper()
	encoded, err := x509.MarshalPKIXPublicKey(key)
	if err != nil {
		t.Fatal(err)
	}
	return encoded
}
