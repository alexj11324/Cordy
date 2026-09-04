package handler

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/auth"
	"github.com/patchbay-ai/patchbay/server/internal/testutil"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func identityAttempt(t *testing.T, h *Handler) desktopAuthHandoffRedeemRequest {
	t.Helper()
	raw, err := generateDesktopHandoffCode()
	if err != nil {
		t.Fatal(err)
	}
	req := desktopAuthHandoffRedeemRequest{State: strings.TrimPrefix(raw, "pbd_"), CodeVerifier: strings.Repeat("v", 43), Code: "pbl_" + strings.TrimPrefix(raw, "pbd_")}
	if err := h.Queries.CreateDesktopAuthHandoff(t.Context(), db.CreateDesktopAuthHandoffParams{State: req.State, CodeChallenge: desktopHandoffCodeChallenge(req.CodeVerifier), CallbackProtocol: "patchbay"}); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_, err := testPool.Exec(context.Background(), "DELETE FROM desktop_auth_handoff WHERE state=$1", req.State)
		if err != nil {
			t.Error(err)
		}
	})
	return req
}

func completedIdentityGrant(t *testing.T, h *Handler) desktopAuthHandoffRedeemRequest {
	t.Helper()
	req := identityAttempt(t, h)
	_, err := h.Queries.CompleteDesktopAuthHandoff(t.Context(), db.CompleteDesktopAuthHandoffParams{State: req.State, CodeChallenge: desktopHandoffCodeChallenge(req.CodeVerifier), UserID: parseUUID(testUserID), CodeHash: pgtype.Text{String: auth.HashToken(req.Code), Valid: true}})
	if err != nil {
		t.Fatal(err)
	}
	return req
}

func TestDesktopLocalIdentityGrantBoundaries(t *testing.T) {
	h := &Handler{Queries: testHandler.Queries}
	req := completedIdentityGrant(t, h)
	call := func(binding desktopAuthHandoffRedeemRequest, status int) *testutil.Response {
		return testutil.Call(t, h.RedeemDesktopLocalIdentity, testutil.JSONRequest(http.MethodPost, "/api/desktop-identity/redeem", binding)).Want(status)
	}
	bad := req
	bad.CodeVerifier = strings.Repeat("x", 43)
	call(bad, 401)
	bad = req
	bad.State = strings.Repeat("s", 43)
	call(bad, 401)
	bad = req
	bad.Code = "pbd_" + strings.TrimPrefix(req.Code, "pbl_")
	call(bad, 401)
	testutil.Call(t, h.RedeemDesktopAuthHandoff, testutil.JSONRequest(http.MethodPost, "/api/desktop-handoff/redeem", req)).Want(401)
	// Changing a local code's prefix cannot redeem it as a production session.
	testutil.Call(t, h.RedeemDesktopAuthHandoff, testutil.JSONRequest(http.MethodPost, "/api/desktop-handoff/redeem", bad)).Want(401)
	response := call(req, 200)
	var identity map[string]any
	response.JSON(&identity)
	if identity["email"] != handlerTestEmail || identity["token"] != nil || len(identity) != 3 {
		t.Fatalf("unexpected identity response: fields=%v", identity)
	}
	if response.Header().Get("Cache-Control") != "no-store" {
		t.Fatal("identity response may be cached")
	}
	call(req, 401)
}

func TestDesktopLocalIdentityGrantExpires(t *testing.T) {
	h := &Handler{Queries: testHandler.Queries}
	req := completedIdentityGrant(t, h)
	if _, err := testPool.Exec(t.Context(), "UPDATE desktop_auth_handoff SET completed_at=now()-interval '2 minutes' WHERE state=$1", req.State); err != nil {
		t.Fatal(err)
	}
	testutil.Call(t, h.RedeemDesktopLocalIdentity, testutil.JSONRequest(http.MethodPost, "/api/desktop-identity/redeem", req)).Want(401)
}

func TestDesktopLocalCompletionMintsPurposeBoundCode(t *testing.T) {
	h := &Handler{Queries: testHandler.Queries, cfg: Config{AllowSignup: true}, ClerkAuth: clerkVerifierFunc(func(context.Context, string, time.Time) (ClerkIdentity, error) {
		return ClerkIdentity{Email: handlerTestEmail}, nil
	})}
	req := identityAttempt(t, h)
	request := testutil.JSONRequest(http.MethodPost, "/api/desktop-google/complete", desktopGoogleAttemptRequest{State: req.State, CodeChallenge: desktopHandoffCodeChallenge(req.CodeVerifier), Local: true})
	request.Header.Set(authContractHeader, authContractVersion)
	request.Header.Set("Authorization", "Bearer fixture-token")
	var response map[string]string
	testutil.Call(t, h.CompleteDesktopGoogleAttempt, request).Want(200).JSON(&response)
	if !desktopLocalIdentityCodePattern.MatchString(response["code"]) {
		t.Fatal("local completion returned a production code")
	}
}

func TestLocalDesktopRequiresInitiationAndConsumesOnce(t *testing.T) {
	calls := 0
	h := &Handler{Queries: testHandler.Queries, TxStarter: testPool, cfg: Config{AllowSignup: true}, redeemDesktopIdentity: func(ctx context.Context, req desktopAuthHandoffRedeemRequest) (ClerkIdentity, error) {
		calls++
		return ClerkIdentity{Email: handlerTestEmail}, nil
	}}
	req := identityAttempt(t, h)
	call := func(binding desktopAuthHandoffRedeemRequest, status int) *testutil.Response {
		return testutil.Call(t, h.RedeemDesktopAuthHandoff, testutil.JSONRequest(http.MethodPost, "/api/desktop-handoff/redeem", binding)).Want(status)
	}
	bad := req
	bad.State = ""
	call(bad, 401)
	bad = req
	bad.State = strings.Repeat("x", 43)
	call(bad, 401)
	bad = req
	bad.CodeVerifier = strings.Repeat("x", 43)
	call(bad, 401)
	if calls != 0 {
		t.Fatal("unbound request reached identity authority")
	}
	var session map[string]string
	call(req, 200).JSON(&session)
	if session["token"] == "" {
		t.Fatal("local session missing")
	}
	call(req, 401)
	if calls != 1 {
		t.Fatalf("identity exchanges = %d, want 1", calls)
	}
}

func TestLocalDesktopExpiredAttemptNeverContactsAuthority(t *testing.T) {
	h := &Handler{Queries: testHandler.Queries, TxStarter: testPool, redeemDesktopIdentity: func(context.Context, desktopAuthHandoffRedeemRequest) (ClerkIdentity, error) {
		t.Fatal("expired attempt reached authority")
		return ClerkIdentity{}, nil
	}}
	req := identityAttempt(t, h)
	if _, err := testPool.Exec(t.Context(), "UPDATE desktop_auth_handoff SET expires_at=now()-interval '1 second' WHERE state=$1", req.State); err != nil {
		t.Fatal(err)
	}
	testutil.Call(t, h.RedeemDesktopAuthHandoff, testutil.JSONRequest(http.MethodPost, "/api/desktop-handoff/redeem", req)).Want(401)
}

func TestDesktopIdentityRejectsMalformedResponses(t *testing.T) {
	for _, body := range []string{`{"email":"user@example.com","name":"Name","avatar_url":"","token":"forbidden"}`, `{"email":"bad","name":"Name","avatar_url":""}`, strings.Repeat("x", 8193), `{"email":"user@example.com","name":"Name","avatar_url":""}{}`} {
		t.Run("reject", func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = w.Write([]byte(body)) }))
			defer server.Close()
			if _, err := requestDesktopIdentity(t.Context(), desktopAuthHandoffRedeemRequest{}, server.Client(), server.URL); err == nil {
				t.Fatal("malformed identity accepted")
			}
		})
	}
}

func TestDesktopIdentityRequestContainsOnlyGrantAndProof(t *testing.T) {
	req := desktopAuthHandoffRedeemRequest{Code: "pbl_" + strings.Repeat("a", 43), State: strings.Repeat("s", 43), CodeVerifier: strings.Repeat("v", 43)}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var got desktopAuthHandoffRedeemRequest
		if r.Header.Get("Authorization") != "" || json.NewDecoder(r.Body).Decode(&got) != nil || got != req {
			t.Error("unexpected identity request")
		}
		_ = json.NewEncoder(w).Encode(desktopIdentityResponse{Email: "user@example.com", Name: "User"})
	}))
	defer server.Close()
	identity, err := requestDesktopIdentity(t.Context(), req, server.Client(), server.URL)
	if err != nil || identity.Email != "user@example.com" {
		t.Fatalf("identity=%+v error=%v", identity, err)
	}
}

func TestDesktopIdentityBoundsRequestBody(t *testing.T) {
	h := &Handler{}
	body := `{"code":"pbl_` + strings.Repeat("a", 43) + `","code_verifier":"` + strings.Repeat("v", 43) + `","state":"` + strings.Repeat("s", 43) + `"}` + strings.Repeat(" ", 5000)
	testutil.Call(t, h.RedeemDesktopLocalIdentity, testutil.JSONRequest(http.MethodPost, "/api/desktop-identity/redeem", body)).Want(401)
}

func TestDesktopIdentityDistinguishesRejectionFromUnavailable(t *testing.T) {
	for _, status := range []int{401, 403, 429, 500, 502} {
		t.Run(http.StatusText(status), func(t *testing.T) {
			upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(status) }))
			defer upstream.Close()
			_, err := requestDesktopIdentity(t.Context(), desktopAuthHandoffRedeemRequest{}, upstream.Client(), upstream.URL)
			want := errDesktopIdentityUnavailable
			if status == 401 {
				want = errDesktopIdentityRejected
			}
			if !errors.Is(err, want) {
				t.Fatalf("status %d: got %v, want %v", status, err, want)
			}
		})
	}
}
