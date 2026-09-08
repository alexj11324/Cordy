package linearsync

import (
	"go/parser"
	"go/token"
	"os"
	"strconv"
	"strings"
	"testing"
)

func TestLinearSyncKeepsHTTPAndServiceOutsideModule(t *testing.T) {
	files, err := os.ReadDir(".")
	if err != nil {
		t.Fatal(err)
	}
	for _, file := range files {
		if file.IsDir() || !strings.HasSuffix(file.Name(), ".go") || strings.HasSuffix(file.Name(), "_test.go") {
			continue
		}
		source, err := parser.ParseFile(token.NewFileSet(), file.Name(), nil, parser.ImportsOnly)
		if err != nil {
			t.Fatal(err)
		}
		for _, spec := range source.Imports {
			importPath, err := strconv.Unquote(spec.Path.Value)
			if err != nil {
				t.Fatal(err)
			}
			for _, boundary := range []string{
				"github.com/orvilo-ai/orvilo/server/internal/handler",
				"github.com/orvilo-ai/orvilo/server/internal/service",
				"net/http",
			} {
				if importPath == boundary || strings.HasPrefix(importPath, boundary+"/") {
					t.Errorf("%s imports %s; HTTP adapters and provider transport belong outside linearsync", file.Name(), importPath)
				}
			}
		}
	}
}
