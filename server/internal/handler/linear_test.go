package handler

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/featureflags"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
	"github.com/patchbay-ai/patchbay/server/pkg/featureflag"
)

func linearTestFlags(enabled bool) *featureflag.Service {
	provider := featureflag.NewStaticProvider()
	provider.Set(featureflags.LinearInstallationFoundation, featureflag.Rule{Default: enabled})
	return featureflag.NewService(provider)
}

func TestGetLinearConnectionFeatureGate(t *testing.T) {
	h := &Handler{FeatureFlags: linearTestFlags(false)}
	recorder := httptest.NewRecorder()
	h.GetLinearConnection(recorder, httptest.NewRequest(http.MethodGet, "/", nil))
	if recorder.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want %d", recorder.Code, http.StatusNotFound)
	}
}

func TestHandleLinearWebhookRejectsInvalidSignatureBeforeDatabase(t *testing.T) {
	box, err := secretbox.New(make([]byte, secretbox.KeySize))
	if err != nil {
		t.Fatal(err)
	}
	h := &Handler{
		FeatureFlags:        linearTestFlags(true),
		LinearSecretBox:     box,
		LinearClientID:      "client",
		LinearClientSecret:  "secret",
		LinearWebhookSecret: "webhook-secret",
	}
	recorder := httptest.NewRecorder()
	request := httptest.NewRequest(http.MethodPost, "/api/webhooks/linear", strings.NewReader(`{"organizationId":"org"}`))
	request.Header.Set("Linear-Signature", "00")
	h.HandleLinearWebhook(recorder, request)
	if recorder.Code != http.StatusUnauthorized {
		t.Fatalf("status = %d, want %d", recorder.Code, http.StatusUnauthorized)
	}
}
