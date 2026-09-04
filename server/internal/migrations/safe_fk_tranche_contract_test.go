package migrations

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

const safeFKTrancheStem = "528_safe_fk_tranche"

func TestSafeFKTrancheUpIsDropConstraintOnly(t *testing.T) {
	sql := readSafeFKTrancheSQL(t, ".up.sql")
	got := normalizeMigrationSQL(sql)
	want := "ALTER TABLE project_resource DROP CONSTRAINT IF EXISTS project_resource_project_id_fkey; ALTER TABLE comment DROP CONSTRAINT IF EXISTS comment_parent_id_fkey; ALTER TABLE attachment DROP CONSTRAINT IF EXISTS attachment_workspace_id_fkey; ALTER TABLE attachment DROP CONSTRAINT IF EXISTS attachment_issue_id_fkey; ALTER TABLE attachment DROP CONSTRAINT IF EXISTS attachment_comment_id_fkey;"
	if got != want {
		t.Fatalf("%s up migration must contain only the five approved idempotent drops; got %q", safeFKTrancheStem, got)
	}

	forbidden := regexp.MustCompile("(?i)\\b(DELETE|TRUNCATE|UPDATE|INSERT|MERGE|DO|EXECUTE)\\b")
	if forbidden.MatchString(stripSQLComments(sql)) {
		t.Fatalf("%s up migration contains a data mutation or dynamic execution", safeFKTrancheStem)
	}
}

func TestDeleteIssueExplicitlyRemovesOwnedAttachments(t *testing.T) {
	path := filepath.Join(filepath.Dir(realMigrationsDir(t)), "pkg", "db", "queries", "issue.sql")
	source, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read issue query: %v", err)
	}

	sql := normalizeMigrationSQL(stripSQLComments(string(source)))
	for _, fragment := range []string{
		"cleared_attachments AS ( DELETE FROM attachment WHERE workspace_id = $2",
		"issue_id IN (SELECT target.id FROM target)",
		"comment.workspace_id = $2 AND comment.issue_id IN (SELECT target.id FROM target)",
		"agent_task_queue.issue_id IN (SELECT target.id FROM target)",
		"(SELECT count(*) FROM cleared_attachments) >= 0",
	} {
		if !strings.Contains(sql, fragment) {
			t.Errorf("DeleteIssue query is missing explicit attachment cleanup fragment %q", fragment)
		}
	}
}

func TestSafeFKTrancheDownDoesNotRestoreForeignKeys(t *testing.T) {
	sql := readSafeFKTrancheSQL(t, ".down.sql")
	got := normalizeMigrationSQL(sql)
	if got != "SELECT 1;" {
		t.Fatalf("%s down migration must be a safe no-op, got %q", safeFKTrancheStem, got)
	}

	if strings.Contains(strings.ToUpper(got), "ADD CONSTRAINT") || strings.Contains(strings.ToUpper(got), "REFERENCES") {
		t.Fatalf("%s down migration must not restore a foreign key", safeFKTrancheStem)
	}
}

func TestDeleteProjectExplicitlyDetachesProjectReferences(t *testing.T) {
	path := filepath.Join(filepath.Dir(realMigrationsDir(t)), "pkg", "db", "queries", "project.sql")
	source, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read project query: %v", err)
	}

	sql := normalizeMigrationSQL(stripSQLComments(string(source)))
	for _, fragment := range []string{
		"UPDATE issue SET project_id = NULL WHERE project_id = $1 AND workspace_id = $2 RETURNING id",
		"UPDATE automation SET project_id = NULL WHERE project_id = $1 AND workspace_id = $2 RETURNING id",
		"DELETE FROM project_resource WHERE project_id = $1 AND workspace_id = $2 RETURNING id",
	} {
		if !strings.Contains(sql, fragment) {
			t.Errorf("DeleteProject query is missing explicit cleanup fragment %q", fragment)
		}
	}
}

func TestDeleteCommentExplicitlyRemovesCommentTreeChildren(t *testing.T) {
	path := filepath.Join(filepath.Dir(realMigrationsDir(t)), "pkg", "db", "queries", "comment.sql")
	source, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read comment query: %v", err)
	}

	sql := normalizeMigrationSQL(stripSQLComments(string(source)))
	for _, fragment := range []string{
		"WITH RECURSIVE locked_issue AS MATERIALIZED",
		"comment_tree(id, issue_id, workspace_id, path) AS MATERIALIZED",
		"UPDATE agent_task_queue task SET trigger_comment_id = NULL WHERE task.trigger_comment_id IN (SELECT id FROM comment_tree)",
		"DELETE FROM comment_reaction WHERE comment_id IN (SELECT id FROM comment_tree)",
		"DELETE FROM attachment WHERE comment_id IN (SELECT id FROM comment_tree)",
		"DELETE FROM comment WHERE comment.id IN (SELECT id FROM comment_tree)",
	} {
		if !strings.Contains(sql, fragment) {
			t.Errorf("DeleteComment query is missing explicit cleanup fragment %q", fragment)
		}
	}
}

func readSafeFKTrancheSQL(t *testing.T, suffix string) string {
	t.Helper()

	path := filepath.Join(realMigrationsDir(t), safeFKTrancheStem+suffix)
	source, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}
	return string(source)
}

func normalizeMigrationSQL(sql string) string {
	return strings.Join(strings.Fields(stripSQLComments(sql)), " ")
}

func stripSQLComments(sql string) string {
	blockComments := regexp.MustCompile("(?s)/\\*.*?\\*/")
	sql = blockComments.ReplaceAllString(sql, "")

	lines := strings.Split(sql, "\n")
	for i, line := range lines {
		if commentStart := strings.Index(line, "--"); commentStart >= 0 {
			lines[i] = line[:commentStart]
		}
	}
	return strings.Join(lines, "\n")
}
