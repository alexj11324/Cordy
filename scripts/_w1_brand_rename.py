#!/usr/bin/env python3
"""One-shot W1 brand rename: Multica -> Patchbay. Do not keep as a product tool."""

from __future__ import annotations

import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SKIP_DIRS = {
    ".git",
    "node_modules",
    ".pnpm-store",
    ".turbo",
    "dist",
    ".next",
}

SKIP_SUFFIXES = {
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".webp",
    ".ico",
    ".woff",
    ".woff2",
    ".ttf",
    ".eot",
    ".mp4",
    ".zip",
    ".gz",
    ".tar",
    ".dylib",
    ".so",
    ".bin",
}

# Longest / most specific first.
REPLACEMENTS: list[tuple[str, str]] = [
    ("github.com/multica-ai/multica/server", "github.com/patchbay-ai/patchbay/server"),
    ("github.com/multica-ai/multica", "github.com/patchbay-ai/patchbay"),
    ("github.com/multica-ai/scoop-bucket", "github.com/patchbay-ai/scoop-bucket"),
    ("ghcr.io/multica-ai", "ghcr.io/patchbay-ai"),
    ("oci://ghcr.io/multica-ai", "oci://ghcr.io/patchbay-ai"),
    ("raw.githubusercontent.com/multica-ai/multica", "raw.githubusercontent.com/patchbay-ai/patchbay"),
    ("https://multica.ai", "https://patchbay.aspectlylabs.com"),
    ("http://multica.ai", "https://patchbay.aspectlylabs.com"),
    ("multica.ai", "patchbay.aspectlylabs.com"),
    ("@multica/", "@patchbay/"),
    ("multica:plugin-bridge-connect", "patchbay:plugin-bridge-init"),
    ("multica:plugin-surface-error", "patchbay:plugin-surface-error"),
    ("multica:plugin-surface-navigated", "patchbay:plugin-surface-navigated"),
    ("multica:plugin-surface-navigation-blocked", "patchbay:plugin-surface-navigation-blocked"),
    ("/api/plugin-bridge/v1", "/api/v1/plugin"),
    ("plugin-bridge/v1", "v1/plugin"),
    ("multica_logged_in", "patchbay_logged_in"),
    ("multica_auth", "patchbay_auth"),
    ("multica_csrf", "patchbay_csrf"),
    ("~/.multica", "~/.patchbay"),
    ("$HOME/.multica", "$HOME/.patchbay"),
    ("%USERPROFILE%\\.multica", "%USERPROFILE%\\.patchbay"),
    ("/.multica/", "/.patchbay/"),
    ("multica://", "patchbay://"),
    ("MULTICA_", "PATCHBAY_"),
    ("_MULTICA", "_PATCHBAY"),
    ("Multica Cloud", "Patchbay Cloud"),
    ("Multica Desktop", "Patchbay Desktop"),
    ("Multica CLI", "Patchbay CLI"),
    ("Multica Public API", "Patchbay Public API"),
    ("Multica Plugin", "Patchbay Plugin"),
    ("a Multica", "a Patchbay"),
    ("A Multica", "A Patchbay"),
    ("the Multica", "the Patchbay"),
    ("The Multica", "The Patchbay"),
    ("to Multica", "to Patchbay"),
    ("into Multica", "into Patchbay"),
    ("from Multica", "from Patchbay"),
    ("with Multica", "with Patchbay"),
    ("for Multica", "for Patchbay"),
    ("of Multica", "of Patchbay"),
    ("on Multica", "on Patchbay"),
    ("in Multica", "in Patchbay"),
    ("is Multica", "is Patchbay"),
    ("as Multica", "as Patchbay"),
    ("Multica's", "Patchbay's"),
    ("Multica’s", "Patchbay’s"),
    ("MULTICA", "PATCHBAY"),
    ("Multica", "Patchbay"),
    ("multica-cli", "patchbay-cli"),
    ("multica-ai/tap/multica", "patchbay-ai/tap/patchbay"),
    ("multica-ai/tap", "patchbay-ai/tap"),
    ("mul_", "pby_"),
]


def is_text_file(path: Path) -> bool:
    if path.suffix.lower() in SKIP_SUFFIXES:
        return False
    try:
        with path.open("rb") as fh:
            chunk = fh.read(8192)
    except OSError:
        return False
    if b"\0" in chunk:
        return False
    return True


def iter_files() -> list[Path]:
    out: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            if name == "_w1_brand_rename.py":
                continue
            path = Path(dirpath) / name
            if is_text_file(path):
                out.append(path)
    return out


def replace_text(content: str) -> str:
    for old, new in REPLACEMENTS:
        content = content.replace(old, new)
    # Remaining lowercase product identifier after the specific cases above.
    content = content.replace("multica", "patchbay")
    return content


def main() -> None:
    changed = 0
    for path in iter_files():
        original = path.read_text(encoding="utf-8", errors="surrogateescape")
        updated = replace_text(original)
        if updated != original:
            path.write_text(updated, encoding="utf-8", errors="surrogateescape")
            changed += 1
            print(f"updated {path.relative_to(ROOT)}")
    print(f"rewrote {changed} files")


if __name__ == "__main__":
    main()
