package handler

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestDecodeDeviceAuthorizationJSONBodyBoundsAndRejectsTrailing(t *testing.T) {
	largePayload := `{"client_name":"cli","padding":"` + strings.Repeat("x", deviceAuthorizationJSONBodyLimit) + `"}`
	cases := []struct {
		name    string
		body    string
		wantErr bool
	}{
		{name: "single document", body: `{"client_name":"cli"}`},
		{name: "trailing JSON", body: `{"client_name":"cli"}{"extra":true}`, wantErr: true},
		{name: "body over limit", body: largePayload, wantErr: true},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodPost, "/api/auth/device/code", strings.NewReader(tc.body))
			writer := httptest.NewRecorder()
			var payload DeviceAuthorizationCodeRequest
			err := decodeJSONBody(writer, req, &payload)
			if (err != nil) != tc.wantErr {
				t.Fatalf("decodeJSONBody error = %v, wantErr %v", err, tc.wantErr)
			}
			if !tc.wantErr && payload.ClientName != "cli" {
				t.Fatalf("client_name = %q, want cli", payload.ClientName)
			}
		})
	}
}
