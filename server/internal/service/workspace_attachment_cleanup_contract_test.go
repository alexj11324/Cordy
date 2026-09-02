package service

import (
	"strings"
	"testing"
)

func TestListWorkspaceAttachmentURLsQueryIsReadOnlyAndWorkspaceScoped(t *testing.T) {
	query := strings.ToUpper(listWorkspaceAttachmentURLsSQL)
	for _, forbidden := range []string{"DELETE", "TRUNCATE", "UPDATE", "INSERT"} {
		if strings.Contains(query, forbidden) {
			t.Fatalf("workspace attachment URL query contains write keyword %q: %s", forbidden, listWorkspaceAttachmentURLsSQL)
		}
	}
	for _, required := range []string{"A.URL", "A.WORKSPACE_ID = $1", "ORDER BY A.ID"} {
		if !strings.Contains(query, required) {
			t.Fatalf("workspace attachment URL query missing %q: %s", required, listWorkspaceAttachmentURLsSQL)
		}
	}
}
