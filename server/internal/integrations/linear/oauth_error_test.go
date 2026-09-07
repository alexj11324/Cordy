package linear

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestHTTPClientRefreshErrorClassification(t *testing.T) {
	for _, tc := range []struct {
		name   string
		status int
		body   string
		kind   ErrorKind
	}{
		{"invalid grant", http.StatusBadRequest, `{"error":"invalid_grant"}`, ErrorInvalidGrant},
		{"invalid client", http.StatusUnauthorized, `{"error":"invalid_client"}`, ErrorProvider},
		{"rate limited", http.StatusTooManyRequests, `{"error":"invalid_grant"}`, ErrorRateLimited},
		{"upstream failure", http.StatusBadGateway, `{"error":"invalid_grant"}`, ErrorProvider},
		{"request timeout", http.StatusRequestTimeout, `{"error":"invalid_grant"}`, ErrorProvider},
	} {
		t.Run(tc.name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				w.WriteHeader(tc.status)
				_, _ = w.Write([]byte(tc.body))
			}))
			defer server.Close()
			client := NewHTTPClient(server.Client())
			client.TokenURL = server.URL
			if _, err := client.RefreshToken(context.Background(), "refresh", "client", "secret"); !IsKind(err, tc.kind) {
				t.Fatalf("refresh error = %v, want kind %s", err, tc.kind)
			}
		})
	}
}
