package protocol

import "testing"

func TestNormalizeMessageAttributionRequiresExplicitValidRanges(t *testing.T) {
	content := "alpha beta"
	sources, citations := NormalizeMessageAttribution(content,
		[]MessageSource{
			{ID: "docs", URL: "https://example.test/docs", Title: "Docs"},
			{ID: "", URL: "https://example.test/missing-id"},
			{ID: "docs", URL: "https://example.test/docs", Title: "Docs"},
		},
		[]MessageCitation{
			{SourceID: "docs", Start: 6, End: 10},
			{SourceID: "missing", Start: 0, End: 5},
			{SourceID: "docs", Start: 8, End: 20},
		},
	)
	if len(sources) != 1 || sources[0].URL != "https://example.test/docs" {
		t.Fatalf("sources = %#v", sources)
	}
	if len(citations) != 1 || citations[0] != (MessageCitation{SourceID: "docs", Start: 6, End: 10}) {
		t.Fatalf("citations = %#v", citations)
	}
}

func TestNormalizeMessageAttributionDropsAmbiguousSourceIDs(t *testing.T) {
	sources, citations := NormalizeMessageAttribution("done", []MessageSource{
		{ID: "same", URL: "https://example.test/a"},
		{ID: "same", URL: "https://example.test/b"},
	}, []MessageCitation{{SourceID: "same", Start: 0, End: 4}})
	if len(sources) != 0 || len(citations) != 0 {
		t.Fatalf("ambiguous attribution survived: sources=%#v citations=%#v", sources, citations)
	}
}

func TestNormalizeMessageAttributionUsesUTF8ByteBoundaries(t *testing.T) {
	source := []MessageSource{{ID: "unicode", URL: "https://example.test/unicode"}}
	_, citations := NormalizeMessageAttribution("猫", source, []MessageCitation{
		{SourceID: "unicode", Start: 0, End: 3},
		{SourceID: "unicode", Start: 0, End: 1},
	})
	if len(citations) != 1 || citations[0].End != 3 {
		t.Fatalf("citations = %#v", citations)
	}
}
