package devseed

import "testing"

func TestValidateTarget(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name    string
		enabled bool
		dbURL   string
		wantErr bool
	}{
		{
			name:    "worktree database on localhost",
			enabled: true,
			dbURL:   "postgres://orvilo:orvilo@localhost:5432/orvilo_01zp_25?sslmode=disable",
		},
		{
			name:    "main development database on IPv4 loopback",
			enabled: true,
			dbURL:   "postgresql://orvilo:orvilo@127.0.0.1:5432/orvilo",
		},
		{
			name:    "IPv6 loopback",
			enabled: true,
			dbURL:   "postgres://orvilo:orvilo@[::1]:5432/orvilo_feature",
		},
		{
			name:    "explicit opt in is required",
			dbURL:   "postgres://orvilo:orvilo@localhost:5432/orvilo_feature",
			wantErr: true,
		},
		{
			name:    "remote database is rejected",
			enabled: true,
			dbURL:   "postgres://orvilo:orvilo@db.example.com:5432/orvilo",
			wantErr: true,
		},
		{
			name:    "unrelated local database is rejected",
			enabled: true,
			dbURL:   "postgres://orvilo:orvilo@localhost:5432/postgres",
			wantErr: true,
		},
		{
			name:    "non postgres URL is rejected",
			enabled: true,
			dbURL:   "https://localhost/orvilo",
			wantErr: true,
		},
		{
			name:    "malformed URL is rejected",
			enabled: true,
			dbURL:   "://bad",
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			err := ValidateTarget(tt.dbURL, tt.enabled)
			if (err != nil) != tt.wantErr {
				t.Fatalf("ValidateTarget() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

func TestFixtureGraphReferencesSeedIssues(t *testing.T) {
	t.Parallel()

	issues := issueFixtures()
	issueIDs := make(map[string]struct{}, len(issues))
	issueNumbers := make(map[int32]struct{}, len(issues))
	for _, issue := range issues {
		id := fixtureID("issue/" + issue.key)
		if _, exists := issueIDs[id]; exists {
			t.Fatalf("duplicate issue id %s", id)
		}
		issueIDs[id] = struct{}{}
		if _, exists := issueNumbers[issue.number]; exists {
			t.Fatalf("duplicate issue number %d", issue.number)
		}
		issueNumbers[issue.number] = struct{}{}
	}

	if len(issues) != 13 {
		t.Fatalf("issue fixture count = %d, want 13", len(issues))
	}
	if len(graphNodeFixtures) != 5 {
		t.Fatalf("graph node fixture count = %d, want 5", len(graphNodeFixtures))
	}
	if len(graphEdgeFixtures) != 4 {
		t.Fatalf("graph edge fixture count = %d, want 4", len(graphEdgeFixtures))
	}

	for _, node := range graphNodeFixtures {
		if _, exists := issueIDs[fixtureID("issue/"+node.issueKey)]; !exists {
			t.Errorf("graph node %q references missing issue %q", node.tempID, node.issueKey)
		}
	}
	for _, edge := range graphEdgeFixtures {
		if _, exists := issueIDs[fixtureID("issue/"+edge.fromIssueKey)]; !exists {
			t.Errorf("graph edge references missing source issue %q", edge.fromIssueKey)
		}
		if _, exists := issueIDs[fixtureID("issue/"+edge.toIssueKey)]; !exists {
			t.Errorf("graph edge references missing target issue %q", edge.toIssueKey)
		}
	}
}

func TestFixtureIDsAreStable(t *testing.T) {
	t.Parallel()

	if got, want := fixtureID("workspace"), "cee623b2-f616-50c2-90dc-764b4a32e55e"; got != want {
		t.Fatalf("fixtureID(workspace) = %q, want %q", got, want)
	}
	if got, want := fixtureID("issue/layout-cleanup"), "37419265-1775-546a-997e-1148e31fcabf"; got != want {
		t.Fatalf("fixtureID(issue/layout-cleanup) = %q, want %q", got, want)
	}
}
