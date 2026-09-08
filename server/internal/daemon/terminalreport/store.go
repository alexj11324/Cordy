// Package terminalreport persists execution results independently of provider
// execution and of the workspaces that the daemon may collect.
package terminalreport

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/orvilo-ai/orvilo/server/pkg/protocol"
)

// Report is immutable once saved. It contains no authentication credentials;
// the sender uses the daemon's current credential for the same server/profile.
type Report struct {
	TaskID    string                          `json:"task_id"`
	Kind      string                          `json:"kind"`
	Identity  protocol.TerminalReportIdentity `json:"identity"`
	Body      json.RawMessage                 `json:"body"`
	CreatedAt time.Time                       `json:"created_at"`
}

// Store serializes publication and cleanup of report files within one daemon.
// The daemon's existing singleton guard owns the process lifecycle.
type Store struct {
	root string
	mu   sync.Mutex
}

// Directory keeps profiles and server destinations separate. It deliberately
// lives under the profile state directory, never under WorkspacesRoot.
func Directory(profileDir, serverURL string) string {
	sum := sha256.Sum256([]byte(strings.TrimRight(serverURL, "/")))
	return filepath.Join(profileDir, "terminal-reports", hex.EncodeToString(sum[:]))
}

func Open(root string) (*Store, error) {
	if !filepath.IsAbs(root) {
		return nil, errors.New("terminal report directory must be absolute")
	}
	if err := makeDirectory(root); err != nil {
		return nil, fmt.Errorf("create terminal report directory: %w", err)
	}
	if err := os.Chmod(root, 0700); err != nil {
		return nil, fmt.Errorf("protect terminal report directory: %w", err)
	}
	if err := syncDirectory(filepath.Dir(root)); err != nil {
		return nil, err
	}
	return &Store{root: root}, nil
}

func NewReport(taskID, claimFence, kind string, body []byte, now time.Time) (Report, error) {
	digest, err := protocol.TerminalReportDigest(kind, body)
	if err != nil {
		return Report{}, err
	}
	r := Report{
		TaskID: taskID, Kind: kind, Body: append(json.RawMessage(nil), body...), CreatedAt: now.UTC(),
		Identity: protocol.TerminalReportIdentity{ReportID: uuid.NewString(), ClaimFence: claimFence, PayloadSHA256: digest},
	}
	return r, r.validate()
}

func (r Report) validate() error {
	if id, err := uuid.Parse(r.TaskID); err != nil || id.String() != r.TaskID {
		return errors.New("invalid terminal report task id")
	}
	if id, err := uuid.Parse(r.Identity.ReportID); err != nil || id.String() != r.Identity.ReportID {
		return errors.New("invalid terminal report id")
	}
	fence, err := strconv.ParseInt(r.Identity.ClaimFence, 10, 64)
	if err != nil || fence <= 0 || strconv.FormatInt(fence, 10) != r.Identity.ClaimFence {
		return errors.New("invalid terminal report claim fence")
	}
	if r.CreatedAt.IsZero() {
		return errors.New("terminal report has no creation time")
	}
	digest, err := protocol.TerminalReportDigest(r.Kind, r.Body)
	if err != nil {
		return err
	}
	if digest != r.Identity.PayloadSHA256 {
		return errors.New("terminal report digest mismatch")
	}
	return nil
}

func (r Report) filename() string { return r.TaskID + "-" + r.Identity.ClaimFence + ".json" }

// Save publishes the file only after its contents reach stable storage. A
// second save of the same result reuses the original report id. Contradictory
// results for an execution cannot replace the first durable result.
func (s *Store) Save(r Report) (Report, error) {
	if err := r.validate(); err != nil {
		return Report{}, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	path := filepath.Join(s.root, r.filename())
	if existing, err := readReport(path); err == nil {
		if existing.Kind != r.Kind || existing.Identity.PayloadSHA256 != r.Identity.PayloadSHA256 {
			return Report{}, errors.New("conflicting terminal report already persisted")
		}
		return existing, nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return Report{}, fmt.Errorf("read existing terminal report: %w", err)
	}
	// Rejected results remain available for diagnosis and must not be silently
	// republished by another callback for the same execution.
	for _, suffix := range []string{".rejected", ".corrupt"} {
		if _, err := os.Stat(path + suffix); err == nil {
			return Report{}, errors.New("terminal report is retained for diagnosis")
		} else if !errors.Is(err, os.ErrNotExist) {
			return Report{}, err
		}
	}
	data, err := json.Marshal(r)
	if err != nil {
		return Report{}, err
	}
	if err := atomicWrite(path, data); err != nil {
		return Report{}, fmt.Errorf("persist terminal report: %w", err)
	}
	return r, nil
}

// Pending preserves damaged files under a .corrupt suffix and reports the
// error while still returning other valid reports for delivery.
func (s *Store) Pending() ([]Report, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	entries, err := os.ReadDir(s.root)
	if err != nil {
		return nil, err
	}
	var reports []Report
	var failures []error
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}
		path := filepath.Join(s.root, entry.Name())
		r, err := readReport(path)
		if err == nil && entry.Name() != r.filename() {
			err = errors.New("terminal report filename does not match identity")
		}
		if err != nil {
			failures = append(failures, fmt.Errorf("read %s: %w", entry.Name(), err))
			var readFailure *os.PathError
			if errors.As(err, &readFailure) {
				// Unreadable does not mean corrupt. Keep the queue entry so
				// every orphan-recovery attempt stays fenced until I/O or
				// permissions recover and the original result can be read.
				continue
			}
			if moveErr := durableRename(path, path+".corrupt"); moveErr != nil {
				failures = append(failures, moveErr)
			}
			continue
		}
		reports = append(reports, r)
	}
	sort.Slice(reports, func(i, j int) bool { return reports[i].CreatedAt.Before(reports[j].CreatedAt) })
	return reports, errors.Join(failures...)
}

func readReport(path string) (Report, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Report{}, err
	}
	var r Report
	if err := json.Unmarshal(data, &r); err != nil {
		return Report{}, err
	}
	return r, r.validate()
}

func (s *Store) Confirm(r Report) error {
	if err := r.validate(); err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	path := filepath.Join(s.root, r.filename())
	if err := s.matches(path, r); err != nil {
		return err
	}
	if err := os.Remove(path); err != nil {
		return err
	}
	return syncDirectory(s.root)
}

func (s *Store) Reject(r Report) error {
	if err := r.validate(); err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	path := filepath.Join(s.root, r.filename())
	if err := s.matches(path, r); err != nil {
		return err
	}
	return durableRename(path, path+".rejected")
}

func (s *Store) matches(path string, r Report) error {
	current, err := readReport(path)
	if err != nil {
		return err
	}
	if current.Identity != r.Identity {
		return errors.New("terminal report identity changed before acknowledgement")
	}
	return nil
}

func atomicWrite(path string, data []byte) error {
	f, err := os.CreateTemp(filepath.Dir(path), ".terminal-report-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return durableRename(f.Name(), path)
}

func makeDirectory(path string) error {
	info, err := os.Stat(path)
	if err == nil {
		if !info.IsDir() {
			return errors.New("terminal report path is not a directory")
		}
		return nil
	}
	if !errors.Is(err, os.ErrNotExist) {
		return err
	}
	parent := filepath.Dir(path)
	if err := makeDirectory(parent); err != nil {
		return err
	}
	if err := os.Mkdir(path, 0700); err != nil && !errors.Is(err, os.ErrExist) {
		return err
	}
	return syncDirectory(parent)
}
