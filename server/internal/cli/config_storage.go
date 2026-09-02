package cli

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const configLockTimeout = 5 * time.Second

type configDocument map[string]json.RawMessage

type configBaseline struct {
	path  string
	known configDocument
}

func setConfigBaseline(cfg *CLIConfig, path string) error {
	known, err := marshalKnownConfig(*cfg)
	if err != nil {
		return fmt.Errorf("encode CLI config baseline: %w", err)
	}
	cfg.baseline = &configBaseline{path: filepath.Clean(path), known: known}
	return nil
}

func marshalKnownConfig(cfg CLIConfig) (configDocument, error) {
	cfg.baseline = nil
	data, err := json.Marshal(cfg)
	if err != nil {
		return nil, err
	}
	var document configDocument
	if err := json.Unmarshal(data, &document); err != nil {
		return nil, err
	}
	return document, nil
}

func readConfigDocument(path string) (configDocument, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return configDocument{}, nil
		}
		return nil, fmt.Errorf("read CLI config: %w", err)
	}
	var document configDocument
	if err := json.Unmarshal(data, &document); err != nil {
		return nil, fmt.Errorf("parse CLI config: %w", err)
	}
	if document == nil {
		return nil, errors.New("parse CLI config: expected a JSON object")
	}
	return document, nil
}

func saveCLIConfigLocked(path string, cfg CLIConfig) error {
	return withConfigLock(filepath.Join(filepath.Dir(path), ".config.lock"), configLockTimeout, func() error {
		latest, err := readConfigDocument(path)
		if err != nil {
			return err
		}
		desired, err := marshalKnownConfig(cfg)
		if err != nil {
			return fmt.Errorf("encode CLI config: %w", err)
		}

		var baseline configDocument
		if cfg.baseline != nil && cfg.baseline.path == filepath.Clean(path) {
			baseline = cfg.baseline.known
		} else {
			var current CLIConfig
			data, err := json.Marshal(latest)
			if err != nil {
				return fmt.Errorf("encode current CLI config: %w", err)
			}
			if err := json.Unmarshal(data, &current); err != nil {
				return fmt.Errorf("parse current CLI config: %w", err)
			}
			baseline, err = marshalKnownConfig(current)
			if err != nil {
				return fmt.Errorf("encode current CLI config baseline: %w", err)
			}
		}

		applyConfigChanges(latest, baseline, desired)
		return writeConfigDocumentAtomically(path, latest)
	})
}

// applyConfigChanges applies a three-way JSON merge. baseline and desired are
// typed projections, while latest is the complete on-disk document. Recursing
// through objects preserves both unknown future fields and concurrent changes
// to sibling known fields.
func applyConfigChanges(latest, baseline, desired configDocument) {
	keys := make(map[string]struct{}, len(baseline)+len(desired))
	for key := range baseline {
		keys[key] = struct{}{}
	}
	for key := range desired {
		keys[key] = struct{}{}
	}

	for key := range keys {
		before, hadBefore := baseline[key]
		after, hasAfter := desired[key]
		if hadBefore == hasAfter && bytes.Equal(before, after) {
			continue
		}

		beforeObject, beforeIsObject := rawObject(before)
		afterObject, afterIsObject := rawObject(after)
		latestObject, latestIsObject := rawObject(latest[key])
		if latestIsObject && (beforeIsObject || afterIsObject) && (!hasAfter || afterIsObject) {
			if !beforeIsObject {
				beforeObject = configDocument{}
			}
			if !afterIsObject {
				afterObject = configDocument{}
			}
			applyConfigChanges(latestObject, beforeObject, afterObject)
			if len(latestObject) == 0 && !hasAfter {
				delete(latest, key)
			} else if encoded, err := json.Marshal(latestObject); err == nil {
				latest[key] = encoded
			}
			continue
		}

		if hasAfter {
			latest[key] = append(json.RawMessage(nil), after...)
		} else {
			delete(latest, key)
		}
	}
}

func rawObject(raw json.RawMessage) (configDocument, bool) {
	if len(raw) == 0 || bytes.Equal(bytes.TrimSpace(raw), []byte("null")) {
		return nil, false
	}
	var object configDocument
	if err := json.Unmarshal(raw, &object); err != nil || object == nil {
		return nil, false
	}
	return object, true
}

func ensureCLIConfigDirectory(dir string) error {
	private := strings.TrimSpace(os.Getenv(TaskConfigRootEnv)) != ""
	mode := os.FileMode(0o755)
	if private {
		mode = 0o700
	}
	if err := os.MkdirAll(dir, mode); err != nil {
		return fmt.Errorf("create CLI config directory: %w", err)
	}
	if !private {
		return nil
	}
	root, _, err := patchbayConfigRoot()
	if err != nil {
		return fmt.Errorf("resolve task-local CLI config root: %w", err)
	}
	for current := dir; ; current = filepath.Dir(current) {
		if err := restrictConfigDirectory(current); err != nil {
			return fmt.Errorf("restrict task-local CLI config directory: %w", err)
		}
		if current == root {
			break
		}
		parent := filepath.Dir(current)
		if parent == current {
			return fmt.Errorf("task-local CLI config directory %q escapes root %q", dir, root)
		}
	}
	return nil
}

func ensurePrivateProfileDirectory(dir string) error {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return fmt.Errorf("create private Desktop profile directory: %w", err)
	}
	if err := restrictConfigDirectory(dir); err != nil {
		return fmt.Errorf("restrict private Desktop profile directory: %w", err)
	}
	return nil
}

func writeConfigDocumentAtomically(path string, document configDocument) error {
	data, err := json.MarshalIndent(document, "", "  ")
	if err != nil {
		return fmt.Errorf("encode CLI config: %w", err)
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".config-*.json.tmp")
	if err != nil {
		return fmt.Errorf("create temp config file: %w", err)
	}
	tmpPath := tmp.Name()
	removeTemp := true
	defer func() {
		if removeTemp {
			_ = os.Remove(tmpPath)
		}
	}()
	if err := restrictConfigFile(tmpPath); err != nil {
		_ = tmp.Close()
		return fmt.Errorf("restrict temp config file: %w", err)
	}
	if _, err := tmp.Write(append(data, '\n')); err != nil {
		_ = tmp.Close()
		return fmt.Errorf("write temp config file: %w", err)
	}
	if err := tmp.Sync(); err != nil {
		_ = tmp.Close()
		return fmt.Errorf("sync temp config file: %w", err)
	}
	if err := tmp.Close(); err != nil {
		return fmt.Errorf("close temp config file: %w", err)
	}
	if err := replaceConfigFile(tmpPath, path); err != nil {
		return fmt.Errorf("replace config file: %w", err)
	}
	removeTemp = false
	return nil
}

func withConfigLock(lockPath string, timeout time.Duration, operation func() error) error {
	lock, err := acquireConfigLock(lockPath, timeout)
	if err != nil {
		return err
	}
	defer lock.Close()
	defer unlockConfigFile(lock) //nolint:errcheck -- close also releases the OS lock
	return operation()
}

func acquireConfigLock(path string, timeout time.Duration) (*os.File, error) {
	lock, err := openConfigLockFile(path)
	if err != nil {
		return nil, fmt.Errorf("open CLI config lock: %w", err)
	}
	if err := restrictConfigFile(path); err != nil {
		_ = lock.Close()
		return nil, fmt.Errorf("restrict CLI config lock: %w", err)
	}
	deadline := time.Now().Add(timeout)
	for {
		acquired, err := tryLockConfigFile(lock)
		if err != nil {
			_ = lock.Close()
			return nil, fmt.Errorf("lock CLI config: %w", err)
		}
		if acquired {
			return lock, nil
		}
		if timeout <= 0 || !time.Now().Before(deadline) {
			_ = lock.Close()
			return nil, fmt.Errorf("lock CLI config: timed out after %s", timeout)
		}
		remaining := time.Until(deadline)
		pause := 25 * time.Millisecond
		if remaining < pause {
			pause = remaining
		}
		time.Sleep(pause)
	}
}
