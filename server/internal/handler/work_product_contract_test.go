package handler

import "testing"

func TestParseGitHubPRURLRequiresCanonicalPullRequestPath(t *testing.T) {
	tests := []struct {
		name   string
		url    string
		owner  string
		repo   string
		number int32
		wantOK bool
	}{
		{
			name:   "canonical URL with suffix",
			url:    " https://github.com/Acme/Patchbay/pull/42?tab=files ",
			owner:  "acme",
			repo:   "patchbay",
			number: 42,
			wantOK: true,
		},
		{name: "non GitHub host", url: "https://gitlab.com/acme/patchbay/-/merge_requests/42"},
		{name: "issue URL", url: "https://github.com/acme/patchbay/issues/42"},
		{name: "zero number", url: "https://github.com/acme/patchbay/pull/0"},
		{name: "missing repository", url: "https://github.com/acme/pull/42"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			owner, repo, number, err := parseGitHubPRURL(tt.url)
			if (err == nil) != tt.wantOK {
				t.Fatalf("parseGitHubPRURL() error = %v, wantOK=%v", err, tt.wantOK)
			}
			if !tt.wantOK {
				return
			}
			if owner != tt.owner || repo != tt.repo || number != tt.number {
				t.Fatalf("parseGitHubPRURL() = (%q, %q, %d), want (%q, %q, %d)", owner, repo, number, tt.owner, tt.repo, tt.number)
			}
		})
	}
}

func TestNormalizePullRequestAttachStateDefaultsAndRejectsUnknown(t *testing.T) {
	if got, err := normalizePullRequestAttachState(""); err != nil || got != "open" {
		t.Fatalf("empty state = (%q, %v), want (open, nil)", got, err)
	}
	if got, err := normalizePullRequestAttachState(" MERGED "); err != nil || got != "merged" {
		t.Fatalf("merged state = (%q, %v), want (merged, nil)", got, err)
	}
	if _, err := normalizePullRequestAttachState("ready"); err == nil {
		t.Fatal("unknown state must be rejected")
	}
}

func TestWorkProductRelationSourceIsExplicitOnly(t *testing.T) {
	for _, source := range []string{
		workProductRelationSourceManual,
		workProductRelationSourceTask,
		workProductRelationSourceDiscovery,
	} {
		if !isExplicitWorkProductRelationSource(source) {
			t.Errorf("source %q was not recognized as explicit", source)
		}
	}
	for _, source := range []string{"provider_discovery", "provider_reference", ""} {
		if isExplicitWorkProductRelationSource(source) {
			t.Errorf("legacy/provider source %q must not be a writable association source", source)
		}
	}
}
