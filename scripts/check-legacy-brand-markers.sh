#!/usr/bin/env bash
set -euo pipefail

pattern='(?<![A-Za-z0-9_])(?:MUL|Mul|mul|CORDY|Cordy|cordy|CODY|Cody|cody)(?![A-Za-z0-9_])|(?:MUL|mul|CORDY|cordy|CODY|cody)[-_:0-9]'
# The GitHub repository is currently named Cordy even though Patchbay is the
# product name. Repository URLs and GitHub owner/repo conditions are identity
# data, not product branding. Remove only exact repository tokens before the
# second scan so a line that contains a real repository reference and another
# legacy marker still fails.

text_hits="$({
  LC_ALL=C git grep -n -P "$pattern" -- . \
    ':!migrations/**' \
    ':!scripts/check-legacy-brand-markers.sh' || true
} | LEGACY_PATTERN="$pattern" LC_ALL=C perl -ne '
BEGIN {
  $legacy = qr/$ENV{LEGACY_PATTERN}/;
  $canonical = qr{
    (?<![A-Za-z0-9_-])
    (?:
      github[.]com/alexj11324/Cordy
      | raw[.]githubusercontent[.]com/alexj11324/Cordy
      | api[.]github[.]com/repos/alexj11324/Cordy
      | github[.]com:alexj11324/Cordy
      | alexj11324/Cordy
    )
    (?![A-Za-z0-9_-])
    | (?<![A-Za-z0-9_-])repo:[[:space:]]+Cordy(?![A-Za-z0-9_-])
  }x;
}
s/$canonical//g;
print if index($_, "legacy-brand-compat") < 0 && /$legacy/;
' || true)"

if [[ -n "$text_hits" ]]; then
  printf '%s\n' "$text_hits"
  echo "Unexpected legacy branding remains in tracked text." >&2
  exit 1
fi

path_hits="$(LC_ALL=C git ls-files | LEGACY_PATTERN="$pattern" LC_ALL=C perl -ne '
BEGIN { $legacy = qr/$ENV{LEGACY_PATTERN}/; }
chomp;
print "$_\n" if /$legacy/;
')"
if [[ -n "$path_hits" ]]; then
  printf '%s\n' "$path_hits"
  echo "Legacy branding remains in a tracked path." >&2
  exit 1
fi
