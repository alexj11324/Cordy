#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

canonical_repo='https://github.com/alexj11324/Cordy'
canonical_raw='https://raw.githubusercontent.com/alexj11324/Cordy'
canonical_chart='oci://ghcr.io/alexj11324/charts/patchbay'
canonical_brew='alexj11324/tap/patchbay'

docs=(
  SELF_HOSTING.md
  SELF_HOSTING_AI.md
  CLI_INSTALL.md
  CLI_AND_DAEMON.md
  apps/docs/content/docs/self-host-quickstart.mdx
  apps/docs/content/docs/self-host-quickstart.ja.mdx
  apps/docs/content/docs/self-host-quickstart.ko.mdx
  apps/docs/content/docs/self-host-quickstart.zh.mdx
  apps/docs/content/docs/cli.mdx
  apps/docs/content/docs/cli.ja.mdx
  apps/docs/content/docs/cli.ko.mdx
  apps/docs/content/docs/cli.zh.mdx
)

scripts=(
  scripts/install.sh
  scripts/install.ps1
  scripts/install.test.sh
  scripts/selfhost-wait.sh
)

require_text() {
  local file=$1
  local expected=$2

  if ! grep -Fq -- "$expected" "$file"; then
    echo "Missing expected current self-host link in $file:"
    echo "  $expected"
    exit 1
  fi
}

require_no_text() {
  local file=$1
  local retired=$2

  if grep -Fq -- "$retired" "$file"; then
    echo "Retired self-host link remains in $file:"
    grep -nF -- "$retired" "$file"
    exit 1
  fi
}

require_current_repo_reference() {
  local file=$1

  if ! grep -Fq -- "$canonical_repo" "$file" &&
    ! grep -Fq -- "$canonical_raw" "$file"; then
    echo "Missing canonical Cordy repository reference in $file"
    exit 1
  fi
}

# These exact strings are current executable repository/deployment addresses,
# not product-domain text or historical migration prose. Keep the check limited
# to these concrete hosts so `patchbay.ai` product text, migration wording,
# legacy asset names, and unrelated external integrations remain untouched.
for doc in "${docs[@]}"; do
  require_current_repo_reference "$doc"
done

for surface in "${docs[@]}" "${scripts[@]}"; do
  require_no_text "$surface" 'https://github.com/patchbay-ai/patchbay.git'
  require_no_text "$surface" 'https://github.com/patchbay-ai/patchbay/'
  require_no_text "$surface" 'https://github.com/patchbay-ai/patchbay"'
  require_no_text "$surface" 'https://raw.githubusercontent.com/patchbay-ai/patchbay/'
  require_no_text "$surface" 'https://api.github.com/repos/patchbay-ai/patchbay/'
  require_no_text "$surface" 'oci://ghcr.io/patchbay-ai/charts/patchbay'
  require_no_text "$surface" 'ghcr.io/patchbay-ai/charts'
  require_no_text "$surface" 'patchbay-ai/tap'
  require_no_text "$surface" 'https://github.com/patchbay-ai/scoop-bucket'
  require_no_text "$surface" 'docker-compose.selfhost.yml:'
  require_no_text "$surface" 'server/cmd/server/router.go:'
  require_no_text "$surface" 'LobeHub'
  require_no_text "$surface" 'lobehub'
  require_no_text "$surface" 'server-rs'
  require_no_text "$surface" 'cd patchbay'
done

require_text SELF_HOSTING.md "$canonical_chart"
require_text SELF_HOSTING.md "brew install $canonical_brew"
require_text SELF_HOSTING.md 'git clone https://github.com/alexj11324/Cordy.git'
require_text SELF_HOSTING.md 'cd Cordy'
require_text apps/docs/content/docs/self-host-quickstart.mdx 'git clone --depth 1 https://github.com/alexj11324/Cordy.git'
require_text apps/docs/content/docs/self-host-quickstart.mdx 'cd Cordy'
require_text CLI_INSTALL.md 'brew install alexj11324/tap/patchbay'
require_text CLI_INSTALL.md 'https://github.com/alexj11324/Cordy/releases/download/'
require_text CLI_AND_DAEMON.md 'git clone https://github.com/alexj11324/Cordy.git'
require_text SELF_HOSTING_AI.md 'curl -fsSL https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.sh'
require_text scripts/install.sh 'REPO_URL="https://github.com/alexj11324/Cordy.git"'
require_text scripts/install.sh 'REPO_WEB_URL="https://github.com/alexj11324/Cordy"'
require_text scripts/install.sh 'BREW_PACKAGE="alexj11324/tap/patchbay"'
require_text scripts/install.sh 'brew tap alexj11324/tap'
require_text scripts/install.sh 'https://github.com/alexj11324/Cordy/releases/download/'
require_text scripts/install.ps1 '$RepoUrl       = "https://github.com/alexj11324/Cordy.git"'
require_text scripts/install.ps1 '$RepoWebUrl    = "https://github.com/alexj11324/Cordy"'
require_text scripts/install.ps1 'https://api.github.com/repos/alexj11324/Cordy/releases/latest'
require_text scripts/install.ps1 'https://github.com/alexj11324/Cordy/releases/download/'
require_text scripts/install.test.sh 'https://github.com/alexj11324/Cordy/releases/tag/v0.3.2'
require_text scripts/selfhost-wait.sh 'brew install alexj11324/tap/patchbay'
require_text .github/workflows/release.yml 'OCI_REGISTRY: oci://ghcr.io/${{ github.repository_owner }}/charts'
require_text .goreleaser.yml 'homepage: "https://github.com/alexj11324/Cordy"'

echo "Self-host current-link contract OK: Cordy repo, GHCR chart, Homebrew, docs, and scope guard"
