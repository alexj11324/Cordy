package cli

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

const DesktopProfileHelperArg = "--patchbay-private-desktop-profile"

type desktopProfileRequest struct {
	Action    string `json:"action"`
	Profile   string `json:"profile"`
	ServerURL string `json:"server_url"`
	Token     string `json:"token"`
	UserID    string `json:"user_id"`
}

// RunDesktopProfileHelper applies one Desktop-owned profile mutation from
// stdin. Credentials never enter argv, and the complete read-modify-write is
// protected by the same cross-process lock used by normal CLI config saves.
func RunDesktopProfileHelper(input io.Reader) error {
	if strings.TrimSpace(os.Getenv(TaskConfigRootEnv)) != "" {
		return errors.New("private Desktop profile helper refuses a task-local config root")
	}
	decoder := json.NewDecoder(io.LimitReader(input, 1<<20))
	decoder.DisallowUnknownFields()
	var request desktopProfileRequest
	if err := decoder.Decode(&request); err != nil {
		return fmt.Errorf("parse private Desktop profile helper request: %w", err)
	}
	var trailing any
	if err := decoder.Decode(&trailing); !errors.Is(err, io.EOF) {
		if err == nil {
			return errors.New("parse private Desktop profile helper request: trailing JSON value")
		}
		return fmt.Errorf("parse private Desktop profile helper request: %w", err)
	}
	return applyDesktopProfileRequest(request)
}

func applyDesktopProfileRequest(request desktopProfileRequest) error {
	if err := validateDesktopProfile(request.Profile); err != nil {
		return err
	}
	serverURL := strings.TrimSpace(request.ServerURL)
	token := strings.TrimSpace(request.Token)
	userID := strings.TrimSpace(request.UserID)
	switch request.Action {
	case "configure":
		if serverURL == "" {
			return errors.New("Desktop server URL cannot be empty")
		}
		if token != "" || userID != "" {
			return errors.New("configure request cannot include credentials")
		}
	case "set_credentials":
		if serverURL == "" {
			return errors.New("Desktop server URL cannot be empty")
		}
		if token == "" {
			return errors.New("Desktop token cannot be empty")
		}
		if userID == "" {
			return errors.New("Desktop user id cannot be empty")
		}
	case "clear_credentials":
		if serverURL != "" || token != "" || userID != "" {
			return errors.New("clear_credentials request cannot include values")
		}
	default:
		return fmt.Errorf("unsupported Desktop profile action %q", request.Action)
	}

	path, err := CLIConfigPathForProfile(request.Profile)
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	if err := ensurePrivateProfileDirectory(dir); err != nil {
		return err
	}
	return withConfigLock(filepath.Join(dir, ".config.lock"), configLockTimeout, func() error {
		if request.Action == "clear_credentials" {
			if _, err := os.Stat(path); errors.Is(err, os.ErrNotExist) {
				return nil
			} else if err != nil {
				return fmt.Errorf("stat Desktop profile config: %w", err)
			}
		}
		document, err := readConfigDocument(path)
		if err != nil {
			return err
		}
		switch request.Action {
		case "configure":
			document["server_url"] = mustMarshalConfigValue(serverURL)
		case "set_credentials":
			document["server_url"] = mustMarshalConfigValue(serverURL)
			document["token"] = mustMarshalConfigValue(token)
			document["desktop_user_id"] = mustMarshalConfigValue(userID)
		case "clear_credentials":
			delete(document, "token")
			delete(document, "desktop_user_id")
		}
		return writeConfigDocumentAtomically(path, document)
	})
}

func mustMarshalConfigValue(value string) json.RawMessage {
	encoded, err := json.Marshal(value)
	if err != nil {
		panic(err)
	}
	return encoded
}

func validateDesktopProfile(profile string) error {
	if profile != "desktop" && (!strings.HasPrefix(profile, "desktop-") || len(profile) == len("desktop-")) {
		return errors.New("private Desktop helper requires a Desktop-owned profile")
	}
	if profile == "." || profile == ".." || filepath.IsAbs(profile) || filepath.Clean(profile) != profile || strings.ContainsAny(profile, `/\\`) {
		return errors.New("private Desktop helper requires a valid Desktop-owned profile")
	}
	for _, character := range profile {
		if (character >= 'a' && character <= 'z') || (character >= '0' && character <= '9') || character == '.' || character == '-' {
			continue
		}
		return errors.New("private Desktop helper requires a valid Desktop-owned profile")
	}
	return nil
}
