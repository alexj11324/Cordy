package handler

import (
	"strings"
	"testing"
)

// Every language the endpoint accepts must have an opening. A missing entry
// would render the empty string, and the member's first ever message from Patrick
// would be a blank bubble under three starter cards.
func TestPatrickOnboardingOpeningCoversEveryAcceptedLanguage(t *testing.T) {
	for language := range patrickOnboardingLanguages {
		opening := buildPatrickOnboardingOpening(language, "Patrick", "Venus")
		if strings.TrimSpace(opening) == "" {
			t.Fatalf("language %q has no opening", language)
		}
		if strings.Contains(opening, "%!") {
			t.Fatalf("language %q has a broken format verb: %s", language, opening)
		}
		if !strings.Contains(opening, "Venus") {
			t.Errorf("language %q dropped the workspace name: %s", language, opening)
		}
		if !strings.Contains(opening, "Patrick") {
			t.Errorf("language %q dropped the agent name: %s", language, opening)
		}
		if !strings.Contains(opening, "Patchbay") {
			t.Errorf("language %q never names the product: %s", language, opening)
		}
	}
}

// Owners may rename Patrick, so the opening reads the agent's current display
// name. Hardcoding "Patrick" would have a renamed agent introduce itself under a
// name the member never chose.
func TestPatrickOnboardingOpeningUsesTheCurrentDisplayName(t *testing.T) {
	opening := buildPatrickOnboardingOpening("en", "Ada", "Venus")
	if !strings.Contains(opening, "I'm Ada,") {
		t.Fatalf("opening does not introduce the renamed agent:\n%s", opening)
	}
	if strings.Contains(opening, "Patrick") {
		t.Fatalf("opening still says Patrick after a rename:\n%s", opening)
	}
}

// A blank name is not a state the product should render around: fall back to
// the default rather than emit "I'm , your Chief of Staff".
func TestPatrickOnboardingOpeningFallsBackToTheDefaultName(t *testing.T) {
	opening := buildPatrickOnboardingOpening("en", "   ", "Venus")
	if !strings.Contains(opening, "I'm Patrick,") {
		t.Fatalf("blank name did not fall back to the product default:\n%s", opening)
	}
}

// Workspace names are member-typed and chat renders assistant content as
// markdown, so an unescaped name reformats Patrick's first sentence.
func TestPatrickOnboardingOpeningEscapesMemberTypedNames(t *testing.T) {
	opening := buildPatrickOnboardingOpening("en", "Patrick", "**Ops** `prod`")

	if strings.Contains(opening, "**Ops**") {
		t.Errorf("emphasis in the workspace name survived unescaped:\n%s", opening)
	}
	if strings.Contains(opening, "`prod`") {
		t.Errorf("a code span in the workspace name survived unescaped:\n%s", opening)
	}
	for _, want := range []string{`\*\*Ops\*\*`, "\\`prod\\`"} {
		if !strings.Contains(opening, want) {
			t.Errorf("expected %q in the escaped opening:\n%s", want, opening)
		}
	}
}

// The escaper must not double-escape its own output: a name containing a
// literal backslash stays one backslash to the reader.
func TestEscapeMarkdownInlineHandlesBackslashesFirst(t *testing.T) {
	if got := escapeMarkdownInline(`a\*b`); got != `a\\\*b` {
		t.Fatalf("escapeMarkdownInline(`a\\*b`) = %q, want %q", got, `a\\\*b`)
	}
}

// The opening is the member's introduction to the working model, so all four
// beats have to survive a copy edit: what Patchbay is, who Patrick is, what happens
// next, and the handoff to the starter cards below.
func TestPatrickOnboardingOpeningKeepsItsFourBeats(t *testing.T) {
	opening := buildPatrickOnboardingOpening("en", "Patrick", "Venus")

	for _, beat := range []string{
		"Patchbay is a workspace", // what the product is
		"Chief of Staff",         // who is speaking
		"turn it into an issue",  // what happens next
		"Pick one below",         // the bridge to the cards
	} {
		if !strings.Contains(opening, beat) {
			t.Errorf("opening lost a required beat (%q):\n%s", beat, opening)
		}
	}

	// The cards below name the options; a written menu duplicates them and
	// costs the member a retype where a click would do.
	if strings.Count(opening, "\n-") > 0 {
		t.Errorf("the opening must not list options — that is the cards' job:\n%s", opening)
	}
}
