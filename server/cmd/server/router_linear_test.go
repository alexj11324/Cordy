package main

import (
	"encoding/base64"
	"testing"

	"github.com/patchbay-ai/patchbay/server/internal/analytics"
	"github.com/patchbay-ai/patchbay/server/internal/events"
	"github.com/patchbay-ai/patchbay/server/internal/realtime"
)

func TestLinearWorkerWiringRequiresCompleteCredentials(t *testing.T) {
	t.Setenv("PATCHBAY_LINEAR_SECRET_KEY", base64.StdEncoding.EncodeToString(make([]byte, 32)))
	t.Setenv("LINEAR_CLIENT_ID", "client")
	t.Setenv("LINEAR_CLIENT_SECRET", "secret")
	t.Setenv("LINEAR_WEBHOOK_SECRET", "webhook")
	t.Setenv("PATCHBAY_LINEAR_PULL_IMPORT_ENABLED", "true")
	t.Setenv("PATCHBAY_LINEAR_PUSH_ENABLED", "true")
	_, h := NewRouterWithOptions(nil, realtime.NewHub(), events.New(), analytics.NoopClient{}, nil, RouterOptions{})
	if h.LinearSecretBox == nil || h.LinearWorker == nil {
		t.Fatal("complete Linear credentials did not wire the secret box and worker")
	}
}

func TestLinearWorkerWiringKeepsOAuthWhenWebhookSecretIsMissing(t *testing.T) {
	t.Setenv("PATCHBAY_LINEAR_SECRET_KEY", base64.StdEncoding.EncodeToString(make([]byte, 32)))
	t.Setenv("LINEAR_CLIENT_ID", "client")
	t.Setenv("LINEAR_CLIENT_SECRET", "secret")
	t.Setenv("LINEAR_WEBHOOK_SECRET", "")
	_, h := NewRouterWithOptions(nil, realtime.NewHub(), events.New(), analytics.NoopClient{}, nil, RouterOptions{})
	if h.LinearSecretBox == nil || h.LinearWorker == nil {
		t.Fatal("missing webhook secret must not disable OAuth and polling")
	}
}
