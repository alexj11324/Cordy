package middleware

import (
	"net/http"
	"strings"

	"github.com/patchbay-ai/patchbay/server/internal/auth"
	publicapiv1 "github.com/patchbay-ai/patchbay/server/pkg/publicapi/v1"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

// PluginBearerOnly keeps a route on a machine-credential trust boundary.
// Prefix matching is only routing. The Action handler still resolves the token
// against its store and rejects an invalid or expired credential.
func PluginBearerOnly(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !IsPluginBearerToken(BearerToken(r)) {
			publicapiv1.WriteProblem(w, r, http.StatusUnauthorized, "plugin_bearer_required", "plugin bearer token required")
			return
		}
		next.ServeHTTP(w, r)
	})
}

// PluginAuth lets the Action API be reached two ways without weakening either.
// Plugin bearer tokens skip the user session chain and are resolved by the
// handler. Every other request runs through ordinary Auth.
func PluginAuth(queries *db.Queries, patCache *auth.PATCache, cloudPAT *auth.CloudPATVerifier) func(http.Handler) http.Handler {
	userAuth := Auth(queries, patCache, cloudPAT)
	return func(next http.Handler) http.Handler {
		sessionHandler := userAuth(next)
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if IsPluginBearerToken(BearerToken(r)) {
				next.ServeHTTP(w, r)
				return
			}
			sessionHandler.ServeHTTP(w, r)
		})
	}
}

// BearerToken pulls the raw credential out of an Authorization header.
func BearerToken(r *http.Request) string {
	header := strings.TrimSpace(r.Header.Get("Authorization"))
	if header == "" {
		return ""
	}
	const prefix = "Bearer "
	if len(header) <= len(prefix) || !strings.EqualFold(header[:len(prefix)], prefix) {
		return ""
	}
	return strings.TrimSpace(header[len(prefix):])
}

// IsPluginBearerToken reports whether a credential is one of ours to resolve.
//
// Prefix-matched rather than validated: this only decides which code path gets
// to look at the token, and an invalid token routed here is refused by the
// handler a moment later. Deciding by prefix keeps a plugin token from being
// tried against the PAT cache and a PAT from being tried against installations.
func IsPluginBearerToken(token string) bool {
	return strings.HasPrefix(token, "mpi_") || strings.HasPrefix(token, "mpc_")
}
