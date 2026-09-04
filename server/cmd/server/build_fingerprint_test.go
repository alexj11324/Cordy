package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestBuildFingerprintMiddleware(t *testing.T) {
	next := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	})

	t.Run("stamped production build", func(t *testing.T) {
		req := httptest.NewRequest(http.MethodGet, "/readyz", nil)
		res := httptest.NewRecorder()
		buildFingerprintMiddleware("sha-a", "0123456789abcdef0123456789abcdef01234567")(next).ServeHTTP(res, req)
		if got := res.Header().Get("X-Patchbay-Build"); got != "sha-a" {
			t.Fatalf("X-Patchbay-Build = %q, want sha-a", got)
		}
		if got := res.Header().Get("X-Patchbay-Commit"); got != "0123456789abcdef0123456789abcdef01234567" {
			t.Fatalf("X-Patchbay-Commit = %q", got)
		}
	})

	t.Run("unstamped development build", func(t *testing.T) {
		req := httptest.NewRequest(http.MethodGet, "/readyz", nil)
		res := httptest.NewRecorder()
		buildFingerprintMiddleware("dev", "unknown")(next).ServeHTTP(res, req)
		if got := res.Header().Get("X-Patchbay-Build"); got != "" {
			t.Fatalf("X-Patchbay-Build = %q, want empty", got)
		}
		if got := res.Header().Get("X-Patchbay-Commit"); got != "" {
			t.Fatalf("X-Patchbay-Commit = %q, want empty", got)
		}
	})
}
