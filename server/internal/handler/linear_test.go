package handler

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/patchbay-ai/patchbay/server/internal/featureflags"
	"github.com/patchbay-ai/patchbay/server/internal/util/secretbox"
	"github.com/patchbay-ai/patchbay/server/pkg/featureflag"
)

func linearTestFlags(enabled bool) *featureflag.Service {
	provider := featureflag.NewStaticProvider()
	provider.Set(featureflags.LinearInstallationFoundation, featureflag.Rule{Default: enabled})
	return featureflag.NewService(provider)
}

func TestValidateLinearWebhookBindsFreshTimestampAndDelivery(t *testing.T) {
	now := time.UnixMilli(1_700_000_000_000).UTC()
	body := []byte(`{"type":"Issue","action":"update","organizationId":"org","webhookId":"hook","webhookTimestamp":1700000000000,"data":{"id":"issue"}}`)
	request := httptest.NewRequest(http.MethodPost,"/",strings.NewReader(string(body)))
	request.Header.Set("Linear-Signature",linearTestSignature(t,"secret",body))
	request.Header.Set("Linear-Timestamp","1700000000000")
	request.Header.Set("Linear-Delivery","delivery")
	event, delivery, err := validateLinearWebhook("secret",request.Header,body,now)
	if err != nil || event.WebhookID != "hook" || delivery != "delivery" { t.Fatalf("event=%+v delivery=%q err=%v",event,delivery,err) }
	request.Header.Set("Linear-Timestamp","1700000000001")
	if _,_,err = validateLinearWebhook("secret",request.Header,body,now); err == nil { t.Fatal("timestamp mismatch was accepted") }
	request.Header.Set("Linear-Timestamp","1700000000000")
	if _,_,err = validateLinearWebhook("secret",request.Header,body,now.Add(61*time.Second)); err == nil { t.Fatal("stale webhook was accepted") }
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
