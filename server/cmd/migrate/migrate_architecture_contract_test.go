package main

import (
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"testing"
)

// Historical migrations are immutable. Apply the current repository DDL rules
// from this refactor's first migration onward, including rollback directions.
func TestArchitectureMigrationsKeepApplicationRelationshipsAndConcurrentIndexes(t *testing.T) {
	paths, err := filepath.Glob(filepath.Join("..", "..", "migrations", "*.sql"))
	if err != nil {
		t.Fatal(err)
	}
	createIndex := regexp.MustCompile(`(?i)\bCREATE\s+(?:UNIQUE\s+)?INDEX\b`)
	foreignKey := regexp.MustCompile(`(?i)\b(?:FOREIGN\s+KEY|REFERENCES)\b`)
	checked := 0
	for _, path := range paths {
		prefix, _, ok := strings.Cut(filepath.Base(path), "_")
		number, err := strconv.Atoi(prefix)
		if !ok || err != nil || number < 598 {
			continue
		}
		checked++
		body, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		body = stripSQLLineComments(body)
		if foreignKey.Match(body) {
			t.Errorf("%s: relationships must be enforced in application transactions", path)
		}
		indexes := createIndex.FindAllIndex(body, -1)
		if len(indexes) == 0 {
			continue
		}
		if len(indexes) != 1 || len(concurrentIndexNamePattern.FindAllIndex(body, -1)) != 1 {
			t.Errorf("%s: each index must be a single CREATE INDEX CONCURRENTLY migration", path)
		}
		// Concurrent builds cannot share a multi-statement simple-query message:
		// PostgreSQL would place it in an implicit transaction and reject it.
		statement := strings.TrimSuffix(strings.TrimSpace(string(body)), ";")
		if strings.Contains(statement, ";") {
			t.Errorf("%s: concurrent index build shares its file with another statement", path)
		}
	}
	if checked < 6 {
		t.Fatalf("expected receipt and two index migrations in both directions, found %d files", checked)
	}
}
