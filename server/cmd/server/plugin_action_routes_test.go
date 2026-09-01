package main

import (
	"encoding/json"
	"io"
	"net/http"
	"testing"

	publicapiv1 "github.com/patchbay-ai/patchbay/server/pkg/publicapi/v1"
)

func TestPluginActionRouteTrustBoundaries(t *testing.T) {
	tests := []struct {
		name          string
		method        string
		path          string
		authorization string
		wantStatus    int
		wantHandler   bool
		wantProblem   bool
	}{
		{
			name:       "legacy /v1 prefix is not routed",
			path:       "/v1/context",
			wantStatus: http.StatusNotFound,
		},
		{
			name:       "unified plugin API rejects a missing token",
			path:       "/api/v1/plugin/context",
			wantStatus: http.StatusUnauthorized,
		},
		{
			name:          "unified plugin API accepts a browser session",
			path:          "/api/v1/plugin/context",
			authorization: "Bearer " + testToken,
			wantHandler:   true,
		},
		{
			name:          "unified plugin API passes plugin tokens to the Action handler",
			path:          "/api/v1/plugin/context",
			authorization: "Bearer mpi_invalid",
			wantHandler:   true,
		},
		{
			name:          "empty resource path uses the problem contract",
			path:          "/api/v1/plugin/issues/",
			authorization: "Bearer mpi_invalid",
			wantStatus:    http.StatusNotFound,
			wantProblem:   true,
		},
		{
			name:          "unsupported method uses the problem contract",
			method:        http.MethodPut,
			path:          "/api/v1/plugin/context",
			authorization: "Bearer mpi_invalid",
			wantStatus:    http.StatusMethodNotAllowed,
			wantProblem:   true,
		},
		{
			name:          "person-triggered hooks stay on the unified prefix",
			path:          "/api/v1/plugin/hooks/summarize",
			authorization: "Bearer " + testToken,
			wantStatus:    http.StatusMethodNotAllowed,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			method := tt.method
			if method == "" {
				method = http.MethodGet
			}
			req, err := http.NewRequest(method, testServer.URL+tt.path, nil)
			if err != nil {
				t.Fatalf("build request: %v", err)
			}
			if tt.authorization != "" {
				req.Header.Set("Authorization", tt.authorization)
			}
			response, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatalf("request failed: %v", err)
			}
			defer response.Body.Close()
			body, _ := io.ReadAll(response.Body)

			if tt.wantHandler {
				if response.StatusCode == http.StatusUnauthorized || response.StatusCode == http.StatusNotFound {
					t.Fatalf("status = %d, request did not reach the Action handler", response.StatusCode)
				}
				return
			}
			if response.StatusCode != tt.wantStatus {
				t.Fatalf("status = %d, want %d", response.StatusCode, tt.wantStatus)
			}
			if tt.wantProblem {
				if got := response.Header.Get("Content-Type"); got != publicapiv1.ProblemContentType {
					t.Fatalf("Content-Type = %q, body=%s", got, body)
				}
				var problem publicapiv1.Problem
				if err := json.Unmarshal(body, &problem); err != nil {
					t.Fatalf("decode problem: %v; body=%s", err, body)
				}
				if problem.Status != tt.wantStatus || problem.RequestID == "" {
					t.Fatalf("unexpected problem: %+v", problem)
				}
			}
		})
	}
}

func TestPluginActionBaseURLPrefersDedicatedVersionedBase(t *testing.T) {
	t.Setenv("PATCHBAY_PLUGIN_API_URL", " https://plugin-api.example.com/api/v1/plugin/ ")

	if got := pluginActionBaseURL("https://api.example.com/"); got != "https://plugin-api.example.com/api/v1/plugin" {
		t.Fatalf("pluginActionBaseURL() = %q", got)
	}
}

func TestPluginActionBaseURLFallsBackToPublicURL(t *testing.T) {
	t.Setenv("PATCHBAY_PLUGIN_API_URL", "")

	if got := pluginActionBaseURL(" https://api.example.com/ "); got != "https://api.example.com/api/v1/plugin" {
		t.Fatalf("pluginActionBaseURL() = %q", got)
	}
}

func TestPluginActionBaseURLOmittedWithoutPublicOrigin(t *testing.T) {
	t.Setenv("PATCHBAY_PLUGIN_API_URL", "")

	if got := pluginActionBaseURL(""); got != "" {
		t.Fatalf("pluginActionBaseURL() = %q, want empty", got)
	}
}
