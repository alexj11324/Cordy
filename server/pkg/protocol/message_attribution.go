package protocol

import (
	"encoding/json"
	"unicode/utf8"
)

// NormalizeMessageAttribution keeps only sources and citations that form a
// valid structured contract for content. It never derives attribution from
// content itself.
func NormalizeMessageAttribution(content string, sources []MessageSource, citations []MessageCitation) ([]MessageSource, []MessageCitation) {
	cleanSources := make([]MessageSource, 0, len(sources))
	sourceByID := make(map[string]MessageSource, len(sources))
	ambiguousIDs := make(map[string]struct{})
	for _, source := range sources {
		if source.ID == "" || source.URL == "" {
			continue
		}
		if existing, exists := sourceByID[source.ID]; exists {
			if existing != source {
				ambiguousIDs[source.ID] = struct{}{}
			}
			continue
		}
		sourceByID[source.ID] = source
	}
	for _, source := range sources {
		existing, exists := sourceByID[source.ID]
		if !exists || existing != source {
			continue
		}
		if _, ambiguous := ambiguousIDs[source.ID]; ambiguous {
			continue
		}
		cleanSources = append(cleanSources, source)
		delete(sourceByID, source.ID)
	}
	sourceIDs := make(map[string]struct{}, len(cleanSources))
	for _, source := range cleanSources {
		sourceIDs[source.ID] = struct{}{}
	}

	cleanCitations := make([]MessageCitation, 0, len(citations))
	for _, citation := range citations {
		if _, exists := sourceIDs[citation.SourceID]; !exists {
			continue
		}
		if citation.Start < 0 || citation.End <= citation.Start || citation.End > len(content) {
			continue
		}
		if !utf8.ValidString(content) ||
			(citation.Start > 0 && !utf8.RuneStart(content[citation.Start])) ||
			(citation.End < len(content) && !utf8.RuneStart(content[citation.End])) {
			continue
		}
		cleanCitations = append(cleanCitations, citation)
	}
	return cleanSources, cleanCitations
}

// DecodeMessageAttribution reads persisted JSONB and applies the same
// referential/range checks used at ingest. Invalid legacy data degrades to
// empty attribution without hiding the message text.
func DecodeMessageAttribution(content string, rawSources, rawCitations []byte) ([]MessageSource, []MessageCitation) {
	var sources []MessageSource
	if err := json.Unmarshal(rawSources, &sources); err != nil {
		sources = nil
	}
	var citations []MessageCitation
	if err := json.Unmarshal(rawCitations, &citations); err != nil {
		citations = nil
	}
	return NormalizeMessageAttribution(content, sources, citations)
}
