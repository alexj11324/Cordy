package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/cobra"
)

const (
	localRunMaxRequestBytes = 256 << 10
	localRunMaxPromptBytes  = 128 << 10
	localRunDefaultTimeout  = 10 * time.Minute
	localRunMaxTimeout      = 30 * time.Minute
	localRunMaxEntries      = 20_000
)

// daemonRunLocalCmd is intentionally hidden. Desktop is the only supported
// caller: it owns the verified bundled CLI path and the renderer IPC boundary.
// This command is deliberately self-contained. It does not load a Patchbay
// profile, read a token, resolve an ambient agent CLI, open a network client,
// or download anything. The first Guest workflow is a bounded local workspace
// inspection, which is useful even on a machine with no cloud account or agent
// installation.
var daemonRunLocalCmd = &cobra.Command{
	Use:    "run-local",
	Short:  "Inspect one local workspace without a Patchbay profile",
	Hidden: true,
	Args:   cobra.NoArgs,
	RunE:   runDaemonRunLocal,
}

type localRunRequest struct {
	WorkingDirectory string `json:"working_directory"`
	Prompt           string `json:"prompt"`
	TimeoutMillis    int64  `json:"timeout_ms,omitempty"`
}

type localRunEvent struct {
	Event      string `json:"event"`
	Text       string `json:"text,omitempty"`
	Status     string `json:"status,omitempty"`
	Error      string `json:"error,omitempty"`
	DurationMs int64  `json:"duration_ms,omitempty"`
}

func init() {
	daemonCmd.AddCommand(daemonRunLocalCmd)
}

func runDaemonRunLocal(cmd *cobra.Command, _ []string) error {
	request, err := decodeLocalRunRequest(cmd.InOrStdin())
	if err != nil {
		return err
	}
	if err := validateLocalRunEnvironment(); err != nil {
		return err
	}
	request, err = normalizeLocalRunRequest(request)
	if err != nil {
		return err
	}

	encoder := json.NewEncoder(cmd.OutOrStdout())
	emit := func(event localRunEvent) error {
		return encoder.Encode(event)
	}
	return runLocalWorkspace(context.Background(), request, emit)
}

func decodeLocalRunRequest(reader io.Reader) (localRunRequest, error) {
	data, err := io.ReadAll(io.LimitReader(reader, localRunMaxRequestBytes+1))
	if err != nil {
		return localRunRequest{}, fmt.Errorf("read local run request: %w", err)
	}
	if len(data) > localRunMaxRequestBytes {
		return localRunRequest{}, errors.New("local run request is too large")
	}

	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	var request localRunRequest
	if err := decoder.Decode(&request); err != nil {
		return localRunRequest{}, fmt.Errorf("decode local run request: %w", err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		if err == nil {
			return localRunRequest{}, errors.New("local run request must contain one JSON object")
		}
		return localRunRequest{}, fmt.Errorf("decode trailing local run request: %w", err)
	}
	return request, nil
}

func normalizeLocalRunRequest(request localRunRequest) (localRunRequest, error) {
	if len([]byte(request.Prompt)) == 0 {
		return localRunRequest{}, errors.New("local prompt must not be empty")
	}
	if len([]byte(request.Prompt)) > localRunMaxPromptBytes {
		return localRunRequest{}, errors.New("local prompt is too large")
	}
	if !filepath.IsAbs(request.WorkingDirectory) {
		return localRunRequest{}, errors.New("local working directory must be absolute")
	}
	resolvedDirectory, err := filepath.EvalSymlinks(filepath.Clean(request.WorkingDirectory))
	if err != nil {
		return localRunRequest{}, fmt.Errorf("resolve local working directory: %w", err)
	}
	info, err := os.Stat(resolvedDirectory)
	if err != nil {
		return localRunRequest{}, fmt.Errorf("stat local working directory: %w", err)
	}
	if !info.IsDir() {
		return localRunRequest{}, errors.New("local working directory is not a directory")
	}
	request.WorkingDirectory = resolvedDirectory

	timeout := localRunDefaultTimeout
	if request.TimeoutMillis != 0 {
		if request.TimeoutMillis < int64(time.Second/time.Millisecond) {
			return localRunRequest{}, errors.New("local timeout is too short")
		}
		timeout = time.Duration(request.TimeoutMillis) * time.Millisecond
	}
	if timeout > localRunMaxTimeout {
		return localRunRequest{}, errors.New("local timeout is too long")
	}
	request.TimeoutMillis = timeout.Milliseconds()
	return request, nil
}

func validateLocalRunEnvironment() error {
	for _, entry := range os.Environ() {
		key, _, _ := strings.Cut(entry, "=")
		if strings.HasPrefix(strings.ToUpper(key), "PATCHBAY_") {
			return fmt.Errorf("local Guest runner refuses Patchbay environment %q", key)
		}
	}
	return nil
}

func runLocalWorkspace(
	parent context.Context,
	request localRunRequest,
	emit func(localRunEvent) error,
) error {
	started := time.Now()
	if err := emit(localRunEvent{Event: "started"}); err != nil {
		return fmt.Errorf("write local run start: %w", err)
	}

	timeout := time.Duration(request.TimeoutMillis) * time.Millisecond
	ctx, cancel := context.WithTimeout(parent, timeout)
	defer cancel()

	if err := emit(localRunEvent{
		Event: "message",
		Text:  fmt.Sprintf("Inspecting %s", request.WorkingDirectory),
	}); err != nil {
		return fmt.Errorf("write local run progress: %w", err)
	}

	entries := 0
	directories := 0
	files := 0
	var totalBytes int64
	err := filepath.WalkDir(request.WorkingDirectory, func(path string, entry os.DirEntry, walkErr error) error {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}
		if walkErr != nil {
			return walkErr
		}
		if path == request.WorkingDirectory {
			return nil
		}
		entries++
		if entries > localRunMaxEntries {
			return errors.New("local workspace contains too many entries")
		}
		if entry.IsDir() {
			directories++
			return nil
		}
		files++
		if info, statErr := entry.Info(); statErr == nil {
			totalBytes += info.Size()
		}
		if files%250 == 0 {
			return emit(localRunEvent{
				Event: "message",
				Text:  fmt.Sprintf("Scanned %d files", files),
			})
		}
		return nil
	})
	status := "completed"
	var runError string
	if err != nil {
		status = "failed"
		if errors.Is(err, context.DeadlineExceeded) {
			status = "timeout"
		}
		runError = err.Error()
	}
	text := fmt.Sprintf("%d files, %d directories, %d bytes", files, directories, totalBytes)
	return emit(localRunEvent{
		Event:      "result",
		Text:       text,
		Status:     status,
		Error:      runError,
		DurationMs: time.Since(started).Milliseconds(),
	})
}
