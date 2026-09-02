package main

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDecodeLocalRunRequestRejectsUnknownAndTrailingFields(t *testing.T) {
	t.Parallel()

	for name, input := range map[string]string{
		"unknown field":   `{"working_directory":"/tmp","prompt":"inspect","timeout_ms":1000,"token":"secret"}`,
		"trailing object": `{"working_directory":"/tmp","prompt":"inspect"}{}`,
	} {
		t.Run(name, func(t *testing.T) {
			if _, err := decodeLocalRunRequest(strings.NewReader(input)); err == nil {
				t.Fatal("decodeLocalRunRequest unexpectedly accepted invalid input")
			}
		})
	}
}

func TestNormalizeLocalRunRequestCanonicalizesDirectoryAndTimeout(t *testing.T) {
	root := t.TempDir()
	request, err := normalizeLocalRunRequest(localRunRequest{
		WorkingDirectory: filepath.Join(root, "."),
		Prompt:           "inspect local files",
	})
	if err != nil {
		t.Fatalf("normalizeLocalRunRequest() error = %v", err)
	}
	if request.WorkingDirectory != root {
		t.Fatalf("working directory = %q, want %q", request.WorkingDirectory, root)
	}
	if request.TimeoutMillis != localRunDefaultTimeout.Milliseconds() {
		t.Fatalf("timeout = %d, want %d", request.TimeoutMillis, localRunDefaultTimeout.Milliseconds())
	}
}

func TestValidateLocalRunEnvironmentRejectsPatchbaySettings(t *testing.T) {
	t.Setenv("PATCHBAY_SERVER_URL", "https://example.invalid")
	if err := validateLocalRunEnvironment(); err == nil {
		t.Fatal("validateLocalRunEnvironment() accepted Patchbay environment")
	}
}

func TestRunLocalWorkspaceStreamsStructuredEvents(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("local"), 0o600); err != nil {
		t.Fatal(err)
	}
	request, err := normalizeLocalRunRequest(localRunRequest{
		WorkingDirectory: root,
		Prompt:           "inspect",
		TimeoutMillis:    5_000,
	})
	if err != nil {
		t.Fatal(err)
	}

	var events []localRunEvent
	if err := runLocalWorkspace(context.Background(), request, func(event localRunEvent) error {
		events = append(events, event)
		return nil
	}); err != nil {
		t.Fatalf("runLocalWorkspace() error = %v", err)
	}
	if len(events) < 3 || events[0].Event != "started" || events[len(events)-1].Event != "result" {
		t.Fatalf("events = %#v, want started/progress/result", events)
	}
	result := events[len(events)-1]
	if result.Status != "completed" || !strings.Contains(result.Text, "1 files") {
		t.Fatalf("result = %#v", result)
	}
	if _, err := json.Marshal(events); err != nil {
		t.Fatalf("events are not JSON encodable: %v", err)
	}
}

func TestRunLocalWorkspaceReportsCancelledContext(t *testing.T) {
	root := t.TempDir()
	request, err := normalizeLocalRunRequest(localRunRequest{
		WorkingDirectory: root,
		Prompt:           "inspect",
		TimeoutMillis:    5_000,
	})
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	var last localRunEvent
	if err := runLocalWorkspace(ctx, request, func(event localRunEvent) error {
		last = event
		return nil
	}); err != nil {
		t.Fatalf("runLocalWorkspace() error = %v", err)
	}
	if last.Event != "result" || last.Status != "failed" {
		t.Fatalf("last event = %#v, want cancelled context failure", last)
	}
}

func TestLocalRunRequestSizeLimit(t *testing.T) {
	data := bytes.Repeat([]byte("x"), localRunMaxRequestBytes+1)
	if _, err := decodeLocalRunRequest(bytes.NewReader(data)); err == nil {
		t.Fatal("decodeLocalRunRequest() accepted an oversized request")
	}
}
