package execenv

import (
	"path/filepath"
	"testing"
)

func TestReadExecutionProvenanceReportsAttachedCheckout(t *testing.T) {
	repo := newTestRepo(t)
	gitRun(t, repo, "remote", "add", "origin", "https://github.com/example/project.git")
	gitRun(t, repo, "checkout", "-b", "feature/provenance")

	got, err := ReadExecutionProvenance(repo)
	if err != nil {
		t.Fatalf("ReadExecutionProvenance: %v", err)
	}
	if got.RepoIdentity != "https://github.com/example/project.git" {
		t.Errorf("RepoIdentity = %q", got.RepoIdentity)
	}
	if got.ExecutionWorkspace != filepath.Clean(repo) {
		t.Errorf("ExecutionWorkspace = %q, want %q", got.ExecutionWorkspace, filepath.Clean(repo))
	}
	if got.HeadBranch != "feature/provenance" {
		t.Errorf("HeadBranch = %q", got.HeadBranch)
	}
	if got.HeadSHA == "" {
		t.Error("HeadSHA is empty")
	}
	if got.HeadState != "attached" {
		t.Errorf("HeadState = %q, want attached", got.HeadState)
	}
}

func TestReadFinalizedExecutionProvenanceReadsDeliveredBranchRef(t *testing.T) {
	repo := newTestRepo(t)
	gitRun(t, repo, "remote", "add", "origin", "git@github.com:example/project.git")
	gitRun(t, repo, "checkout", "-b", "feature/provenance")
	writeFile(t, filepath.Join(repo, "delivered.txt"), "delivered\n")
	gitRun(t, repo, "add", "delivered.txt")
	gitRun(t, repo, "commit", "-m", "deliver")
	wantSHA := gitRun(t, repo, "rev-parse", "HEAD")

	// The worktree path is intentionally independent from the repository root:
	// Finalize removes it before this helper runs, but it remains the ownership
	// anchor sent to the server.
	formerWorktree := filepath.Join(t.TempDir(), "removed-worktree")
	got, err := ReadFinalizedExecutionProvenance(repo, formerWorktree, "feature/provenance")
	if err != nil {
		t.Fatalf("ReadFinalizedExecutionProvenance: %v", err)
	}
	if got.RepoIdentity != "git@github.com:example/project.git" {
		t.Errorf("RepoIdentity = %q", got.RepoIdentity)
	}
	if got.ExecutionWorkspace != filepath.Clean(formerWorktree) {
		t.Errorf("ExecutionWorkspace = %q, want %q", got.ExecutionWorkspace, filepath.Clean(formerWorktree))
	}
	if got.HeadBranch != "feature/provenance" {
		t.Errorf("HeadBranch = %q", got.HeadBranch)
	}
	if got.HeadSHA != wantSHA {
		t.Errorf("HeadSHA = %q, want delivered commit %q", got.HeadSHA, wantSHA)
	}
	if got.HeadState != "attached" {
		t.Errorf("HeadState = %q, want attached", got.HeadState)
	}
}
