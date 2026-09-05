#!/usr/bin/env python3
"""Restricted staging deployment gateway.

Staging is the internal test environment. This gateway accepts the same JSON
protocol as production, then applies images to isolated Compose projects,
ports, volumes, and a dedicated Clerk snapshot. It never inspects or mutates
production state.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen


SCHEMA_VERSION = 1
REPOSITORY = "alexj11324/Cordy"  # legacy-brand-compat: current GitHub repository identity
REPOSITORY_URL = "https://github.com/alexj11324/Cordy.git"  # legacy-brand-compat
DEFAULT_ROOT = Path("/var/lib/patchbay-staging")
DEFAULT_STATIC_DIRECTORY = Path("/usr/local/share/patchbay-staging")
PRODUCTION_ROOT = Path("/var/lib/patchbay-production")
PRODUCTION_STATIC_DIRECTORY = Path("/usr/local/share/patchbay-production")
PRODUCTION_GATEWAY = Path("/usr/local/bin/patchbay-production-deploy")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
WORKFLOW_RUN_ID_RE = re.compile(r"^[1-9][0-9]{0,19}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
COMPOSE_VARIABLE_RE = re.compile(r"\$\{([A-Z][A-Z0-9_]*)")
EXPECTED_IMAGE_REPOSITORIES = {
    "backend": "ghcr.io/alexj11324/patchbay-backend",
    "web": "ghcr.io/alexj11324/patchbay-web",
    "docs": "ghcr.io/alexj11324/patchbay-docs",
    "auth-broker": "ghcr.io/alexj11324/patchbay-auth-broker",
}
PRODUCT_COMPOSE_PROJECT = "patchbay-staging"
DOCS_COMPOSE_PROJECT = "patchbay-staging-docs"
AUTH_BROKER_COMPOSE_PROJECT = "patchbay-staging-auth-broker"
FORBIDDEN_COMPOSE_PROJECTS = {"cordy632", "cordy", "patchbay-auth-broker"}
STAGING_PORTS = {
    "BACKEND_PORT": "8211",
    "FRONTEND_PORT": "3111",
}
STAGING_URLS = {
    "PATCHBAY_PUBLIC_URL": "https://api.staging.aspectlylabs.com",
    "PATCHBAY_APP_URL": "https://staging.aspectlylabs.com",
    "FRONTEND_ORIGIN": "https://staging.aspectlylabs.com",
}
STAGING_BROKER_URLS = {
    "PATCHBAY_API_ORIGIN": "https://api.staging.aspectlylabs.com",
    "PATCHBAY_AUTH_BROKER_ORIGIN": "https://accounts.staging.aspectlylabs.com",
}
PRODUCTION_MARKERS = (
    "://api.aspectlylabs.com",
    "://patchbay.aspectlylabs.com",
    "://accounts.aspectlylabs.com",
    "://accounts-origin.aspectlylabs.com",
    "cordy632",
    "/var/lib/patchbay-production",
    "/usr/local/share/patchbay-production",
    "/usr/local/bin/patchbay-production-deploy",
)
STAGING_SMOKE_USER_EMAIL = "staging-smoke@aspectlylabs.com"
PROBE_PORTS = {
    "backend": 8211,
    "web": 3111,
    "docs": 4001,
    "auth_broker": 43101,
}


class DeploymentError(RuntimeError):
    pass


def log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def run(
    arguments: list[str],
    *,
    env: dict[str, str] | None = None,
    capture: bool = False,
) -> str:
    log(f"+ {' '.join(arguments)}")
    if capture:
        completed = subprocess.run(
            arguments,
            env=env,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        return completed.stdout.strip()
    subprocess.run(
        arguments,
        env=env,
        check=True,
        text=True,
        stdout=sys.stderr,
        stderr=sys.stderr,
    )
    return ""


def require_sha(value: Any, label: str = "source_sha") -> str:
    if not isinstance(value, str) or not SHA_RE.fullmatch(value):
        raise DeploymentError(f"{label} must be a 40-character lowercase Git SHA")
    return value


def require_workflow_run_id(value: Any, label: str = "workflow_run_id") -> str:
    normalized = str(value) if isinstance(value, (str, int)) else ""
    if not WORKFLOW_RUN_ID_RE.fullmatch(normalized):
        raise DeploymentError(f"{label} must be a positive GitHub Actions run ID")
    return normalized


def validate_image_ref(name: str, value: Any, *, immutable: bool = True) -> str:
    repository = EXPECTED_IMAGE_REPOSITORIES[name]
    if not isinstance(value, str):
        raise DeploymentError(f"{name} image reference must be a string")
    if immutable:
        prefix = f"{repository}@"
        if not value.startswith(prefix) or not DIGEST_RE.fullmatch(value[len(prefix) :]):
            raise DeploymentError(f"{name} must use the allow-listed repository and sha256 digest")
    elif not (
        value.startswith(f"{repository}@sha256:")
        or re.fullmatch(re.escape(repository) + r":[A-Za-z0-9_.-]+", value)
    ):
        raise DeploymentError(f"stored {name} image reference is outside the allow-list")
    return value


def validate_deploy_request(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise DeploymentError("deployment request must be a JSON object")
    if value.get("schema_version") != SCHEMA_VERSION or value.get("action") != "deploy":
        raise DeploymentError("unsupported deployment protocol")
    if value.get("repository") != REPOSITORY:
        raise DeploymentError("deployment repository does not match the staging allow-list")
    source_sha = require_sha(value.get("source_sha"))
    images = value.get("images")
    if not isinstance(images, dict) or set(images) != set(EXPECTED_IMAGE_REPOSITORIES):
        raise DeploymentError("deployment must contain exactly backend, web, docs, and auth-broker")
    normalized_images = {
        name: validate_image_ref(name, images[name]) for name in EXPECTED_IMAGE_REPOSITORIES
    }
    return {
        "schema_version": SCHEMA_VERSION,
        "action": "deploy",
        "repository": REPOSITORY,
        "source_sha": source_sha,
        "workflow_run_id": require_workflow_run_id(value.get("workflow_run_id")),
        "images": normalized_images,
        "bootstrap": False,
    }


def validate_stored_manifest(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("schema_version") != SCHEMA_VERSION:
        raise DeploymentError("stored deployment manifest is invalid")
    source_sha = require_sha(value.get("source_sha"))
    images = value.get("images")
    if not isinstance(images, dict) or set(images) != set(EXPECTED_IMAGE_REPOSITORIES):
        raise DeploymentError("stored deployment image set is incomplete")
    bootstrap = value.get("bootstrap") is True
    normalized = dict(value)
    normalized["source_sha"] = source_sha
    normalized["bootstrap"] = bootstrap
    normalized["images"] = {
        name: validate_image_ref(name, images[name], immutable=not bootstrap)
        for name in EXPECTED_IMAGE_REPOSITORIES
    }
    return normalized


def assert_isolated_path(path: Path, *, forbidden: Path, label: str) -> Path:
    resolved = path.resolve()
    forbidden_resolved = forbidden.resolve()
    if resolved == forbidden_resolved or forbidden_resolved in resolved.parents:
        raise DeploymentError(f"{label} must not use the production path {forbidden}")
    return resolved


def assert_isolated_project(name: str) -> str:
    if name in FORBIDDEN_COMPOSE_PROJECTS:
        raise DeploymentError(f"refusing production Compose project {name}")
    return name


def reject_production_markers(values: dict[str, str], *, label: str) -> None:
    for name, value in values.items():
        for marker in PRODUCTION_MARKERS:
            if marker in value:
                raise DeploymentError(
                    f"{label} {name} mentions production marker {marker}"
                )


def require_exact(values: dict[str, str], expected: dict[str, str], *, label: str) -> None:
    for name, want in expected.items():
        got = values.get(name)
        if got != want:
            raise DeploymentError(f"{label} {name} must be {want}, got {got!r}")


def require_string_map(value: Any, *, label: str) -> dict[str, str]:
    if not isinstance(value, dict):
        raise DeploymentError(f"{label} is missing")
    if not all(isinstance(key, str) and isinstance(item, str) for key, item in value.items()):
        raise DeploymentError(f"{label} is invalid")
    return value


def clerk_api_request(
    secret_key: str,
    path: str,
    *,
    payload: dict[str, Any] | None = None,
) -> Any:
    if not secret_key.startswith("sk_"):
        raise DeploymentError("staging Clerk secret is missing or invalid")
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    headers = {
        "Authorization": f"Bearer {secret_key}",
        "Accept": "application/json",
        "User-Agent": "PatchbayStagingDeploy/1",
    }
    if data is not None:
        headers["Content-Type"] = "application/json"
    request = Request(
        f"https://api.clerk.com/v1/{path.lstrip('/')}",
        data=data,
        headers=headers,
        method="POST" if data is not None else "GET",
    )
    try:
        with urlopen(request, timeout=15) as response:
            return json.load(response)
    except HTTPError as error:
        raise DeploymentError(
            f"Clerk browser-acceptance credential request returned HTTP {error.code}"
        ) from error
    except (URLError, TimeoutError, json.JSONDecodeError) as error:
        raise DeploymentError(
            "Clerk browser-acceptance credential request failed"
        ) from error


def clerk_users(value: Any) -> list[dict[str, Any]]:
    candidates = value.get("data") if isinstance(value, dict) else value
    if not isinstance(candidates, list):
        raise DeploymentError("Clerk returned an invalid user-list response")
    return [candidate for candidate in candidates if isinstance(candidate, dict)]


class StagingDeployment:
    def __init__(
        self,
        root: Path = DEFAULT_ROOT,
        static_directory: Path = DEFAULT_STATIC_DIRECTORY,
    ) -> None:
        self.root = assert_isolated_path(root, forbidden=PRODUCTION_ROOT, label="staging root")
        self.static_directory = assert_isolated_path(
            static_directory,
            forbidden=PRODUCTION_STATIC_DIRECTORY,
            label="staging static directory",
        )
        if PRODUCTION_GATEWAY.exists() and self.root == PRODUCTION_ROOT:
            raise DeploymentError("staging gateway refused to share the production root")
        self.repository = self.root / "repository.git"
        self.releases = self.root / "releases"
        self.history = self.root / "history"
        self.secrets = self.root / "secrets"
        self.current_path = self.root / "current.json"
        self.bootstrapped_path = self.root / "bootstrapped.json"

    def initialize_directories(self) -> None:
        for path in (self.root, self.releases, self.history, self.secrets):
            path.mkdir(parents=True, exist_ok=True)

    def atomic_json(self, path: Path, value: dict[str, Any], mode: int = 0o600) -> None:
        temporary = path.with_name(f".{path.name}.tmp-{os.getpid()}")
        temporary.write_text(f"{json.dumps(value, indent=2, sort_keys=True)}\n", encoding="utf-8")
        os.chmod(temporary, mode)
        os.replace(temporary, path)

    def read_json(self, path: Path) -> dict[str, Any] | None:
        if not path.exists():
            return None
        return json.loads(path.read_text(encoding="utf-8"))

    def ensure_repository(self) -> None:
        if not self.repository.exists():
            run(["git", "init", "--bare", str(self.repository)])
            run(
                [
                    "git",
                    "--git-dir",
                    str(self.repository),
                    "remote",
                    "add",
                    "origin",
                    REPOSITORY_URL,
                ]
            )
            return
        run(
            [
                "git",
                "--git-dir",
                str(self.repository),
                "remote",
                "set-url",
                "origin",
                REPOSITORY_URL,
            ]
        )

    def fetch_main(self) -> str:
        self.ensure_repository()
        run(
            [
                "git",
                "--git-dir",
                str(self.repository),
                "fetch",
                "--no-tags",
                "origin",
                "+refs/heads/main:refs/remotes/origin/main",
            ]
        )
        return run(
            ["git", "--git-dir", str(self.repository), "rev-parse", "refs/remotes/origin/main"],
            capture=True,
        )

    def checkout(self, source_sha: str) -> Path:
        release = self.releases / source_sha
        if release.exists():
            actual = run(["git", "-C", str(release), "rev-parse", "HEAD"], capture=True)
            if actual != source_sha:
                raise DeploymentError(f"release directory {release} has unexpected commit {actual}")
            return release
        run(
            [
                "git",
                "--git-dir",
                str(self.repository),
                "worktree",
                "add",
                "--detach",
                str(release),
                source_sha,
            ]
        )
        return release

    def prune_releases(self) -> None:
        retained: set[str] = set()
        raw = self.read_json(self.current_path)
        if raw is not None:
            retained.add(validate_stored_manifest(raw)["source_sha"])

        if not self.releases.exists():
            return
        for release in sorted(self.releases.iterdir()):
            if not SHA_RE.fullmatch(release.name) or release.name in retained:
                continue
            if release.is_symlink() or not release.is_dir():
                raise DeploymentError(f"refusing unsafe release path {release}")
            actual = run(["git", "-C", str(release), "rev-parse", "HEAD"], capture=True)
            if actual != release.name:
                raise DeploymentError(
                    f"refusing to prune release {release}; checked-out commit is {actual}"
                )
            run(
                [
                    "git",
                    "--git-dir",
                    str(self.repository),
                    "worktree",
                    "remove",
                    "--force",
                    str(release),
                ]
            )
        run(["git", "--git-dir", str(self.repository), "worktree", "prune"])

    def load_secrets(self) -> tuple[dict[str, str], dict[str, str]]:
        product = require_string_map(
            self.read_json(self.secrets / "product-env.json"),
            label="staging product environment snapshot",
        )
        broker = require_string_map(
            self.read_json(self.secrets / "auth-broker-env.json"),
            label="staging auth broker environment snapshot",
        )
        reject_production_markers(product, label="staging product")
        reject_production_markers(broker, label="staging auth broker")
        require_exact(product, STAGING_URLS, label="staging product")
        require_exact(product, STAGING_PORTS, label="staging product")
        require_exact(broker, STAGING_BROKER_URLS, label="staging auth broker")
        cookie_domain = product.get("COOKIE_DOMAIN", "")
        if "staging.aspectlylabs.com" not in cookie_domain:
            raise DeploymentError("staging COOKIE_DOMAIN must be scoped to staging.aspectlylabs.com")
        publishable_key = broker.get("CLERK_PUBLISHABLE_KEY")
        if not isinstance(publishable_key, str) or not publishable_key.strip():
            raise DeploymentError("staging auth broker environment must include CLERK_PUBLISHABLE_KEY")
        product["PATCHBAY_CLERK_PUBLISHABLE_KEY"] = publishable_key.strip()
        return product, broker

    def bootstrap(self) -> dict[str, Any]:
        self.initialize_directories()
        self.load_secrets()
        for name in (
            "staging-product.override.yml",
            "staging-docs.compose.yml",
            "staging-auth-broker.compose.yml",
        ):
            if not (self.static_directory / name).is_file():
                raise DeploymentError(f"installed staging deployment file is missing: {name}")
        source_sha = self.fetch_main()
        self.atomic_json(
            self.bootstrapped_path,
            {
                "schema_version": SCHEMA_VERSION,
                "action": "bootstrap",
                "repository": REPOSITORY,
                "source_sha": source_sha,
            },
        )
        return {"ok": True, "action": "bootstrap", "source_sha": source_sha}

    def check(self) -> dict[str, Any]:
        if self.read_json(self.bootstrapped_path) is None:
            raise DeploymentError("staging state is missing; run --bootstrap first")
        self.load_secrets()
        current_raw = self.read_json(self.current_path)
        source_sha = None
        if current_raw is not None:
            source_sha = validate_stored_manifest(current_raw)["source_sha"]
        return {"ok": True, "action": "check", "source_sha": source_sha}

    def deployment_environment(
        self, manifest: dict[str, Any]
    ) -> tuple[dict[str, str], dict[str, str]]:
        product, broker = self.load_secrets()
        product_env = os.environ.copy()
        product_env.update(product)
        product_env["PATCHBAY_BACKEND_IMAGE_REF"] = manifest["images"]["backend"]
        product_env["PATCHBAY_WEB_IMAGE_REF"] = manifest["images"]["web"]
        product_env["PATCHBAY_DOCS_IMAGE_REF"] = manifest["images"]["docs"]
        broker_env = os.environ.copy()
        broker_env.update(broker)
        broker_env["PATCHBAY_AUTH_BROKER_IMAGE"] = manifest["images"]["auth-broker"]
        return product_env, broker_env

    def issue_browser_acceptance_credentials(self) -> dict[str, str]:
        product, _broker = self.load_secrets()
        secret_key = product.get("CLERK_SECRET_KEY")
        if not isinstance(secret_key, str):
            raise DeploymentError("staging Clerk secret is missing")

        query = urlencode(
            [("email_address", STAGING_SMOKE_USER_EMAIL), ("limit", "2")]
        )
        users = clerk_users(clerk_api_request(secret_key, f"users?{query}"))
        exact = []
        for user in users:
            addresses = user.get("email_addresses")
            if not isinstance(addresses, list):
                continue
            if any(
                isinstance(address, dict)
                and address.get("email_address") == STAGING_SMOKE_USER_EMAIL
                for address in addresses
            ):
                exact.append(user)
        if len(exact) != 1 or not isinstance(exact[0].get("id"), str):
            raise DeploymentError(
                "the dedicated staging browser-acceptance Clerk user is missing or ambiguous"
            )
        user_id = exact[0]["id"]
        sign_in = clerk_api_request(
            secret_key,
            "sign_in_tokens",
            payload={"user_id": user_id, "expires_in_seconds": 300},
        )
        testing = clerk_api_request(secret_key, "testing_tokens", payload={})
        sign_in_ticket = sign_in.get("token") if isinstance(sign_in, dict) else None
        testing_token = testing.get("token") if isinstance(testing, dict) else None
        if not isinstance(sign_in_ticket, str) or not sign_in_ticket:
            raise DeploymentError("Clerk did not return a browser sign-in ticket")
        if not isinstance(testing_token, str) or not testing_token:
            raise DeploymentError("Clerk did not return a browser testing token")
        return {
            "sign_in_ticket": sign_in_ticket,
            "testing_token": testing_token,
        }

    def compose(self, arguments: list[str], *, env: dict[str, str]) -> None:
        if "--project-name" in arguments:
            index = arguments.index("--project-name")
            if index + 1 < len(arguments):
                assert_isolated_project(arguments[index + 1])
        run(["docker", "compose", *arguments], env=env)

    def record_runtime_diagnostics(self, source_sha: str) -> None:
        diagnostics: dict[str, Any] = {"source_sha": source_sha, "projects": {}}
        for project in (
            PRODUCT_COMPOSE_PROJECT,
            DOCS_COMPOSE_PROJECT,
            AUTH_BROKER_COMPOSE_PROJECT,
        ):
            try:
                completed = subprocess.run(
                    ["docker", "compose", "--project-name", project, "ps", "--format", "json"],
                    check=False,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    timeout=5,
                )
                diagnostics["projects"][project] = completed.stdout[-32_000:]
            except subprocess.TimeoutExpired:
                diagnostics["projects"][project] = "[diagnostic command timed out]"
        self.atomic_json(
            self.history / f"failed-runtime-{source_sha}.json",
            diagnostics,
        )

    def probe(
        self,
        url: str,
        *,
        expected_build: str | None = None,
        expected_commit: str | None = None,
        headers: dict[str, str] | None = None,
    ) -> None:
        last_error: Exception | None = None
        for _attempt in range(18):
            try:
                request = Request(url, headers=headers or {})
                with urlopen(request, timeout=10) as response:
                    if response.status >= 400:
                        raise DeploymentError(f"{url} returned HTTP {response.status}")
                    if expected_build is not None:
                        actual = response.headers.get("X-Patchbay-Build")
                        if actual != expected_build:
                            raise DeploymentError(
                                f"{url} reported build {actual!r}, expected {expected_build!r}"
                            )
                    if expected_commit is not None:
                        actual = response.headers.get("X-Patchbay-Commit")
                        if actual != expected_commit:
                            raise DeploymentError(
                                f"{url} reported commit {actual!r}, expected {expected_commit!r}"
                            )
                return
            except (
                DeploymentError,
                ConnectionError,
                HTTPError,
                URLError,
                TimeoutError,
            ) as error:
                last_error = error
                time.sleep(5)
        raise DeploymentError(f"probe failed for {url}: {last_error}")

    def apply(self, manifest: dict[str, Any]) -> None:
        source_sha = manifest["source_sha"]
        release = self.checkout(source_sha)
        product_env, broker_env = self.deployment_environment(manifest)
        for image in manifest["images"].values():
            run(["docker", "pull", image])

        product_files = [
            "--project-name",
            assert_isolated_project(PRODUCT_COMPOSE_PROJECT),
            "--project-directory",
            str(release),
            "-f",
            str(release / "docker-compose.selfhost.yml"),
            "-f",
            str(self.static_directory / "staging-product.override.yml"),
        ]
        self.compose([*product_files, "up", "-d", "--wait", "--wait-timeout", "180", "postgres"], env=product_env)
        for service in ("backend", "frontend"):
            self.compose(
                [
                    *product_files,
                    "up",
                    "-d",
                    "--no-deps",
                    "--wait",
                    "--wait-timeout",
                    "300",
                    service,
                ],
                env=product_env,
            )
        self.compose(
            [
                "--project-name",
                assert_isolated_project(DOCS_COMPOSE_PROJECT),
                "-f",
                str(self.static_directory / "staging-docs.compose.yml"),
                "up",
                "-d",
                "--no-deps",
                "--wait",
                "--wait-timeout",
                "180",
                "docs",
            ],
            env=product_env,
        )
        self.compose(
            [
                "--project-name",
                assert_isolated_project(AUTH_BROKER_COMPOSE_PROJECT),
                "-f",
                str(self.static_directory / "staging-auth-broker.compose.yml"),
                "up",
                "-d",
                "--no-deps",
                "--wait",
                "--wait-timeout",
                "180",
                "broker",
            ],
            env=broker_env,
        )

        expected = f"sha-{source_sha}"
        public_host_headers = {
            "Host": "staging.aspectlylabs.com",
            "X-Forwarded-Proto": "https",
        }
        self.probe(
            f"http://127.0.0.1:{PROBE_PORTS['backend']}/readyz",
            expected_build=expected,
            expected_commit=source_sha,
        )
        self.probe(f"http://127.0.0.1:{PROBE_PORTS['backend']}/api/config")
        self.probe(
            f"http://127.0.0.1:{PROBE_PORTS['web']}/login",
            expected_build=expected,
            expected_commit=source_sha,
            headers=public_host_headers,
        )
        self.probe(
            f"http://127.0.0.1:{PROBE_PORTS['docs']}/docs",
            expected_build=expected,
            expected_commit=source_sha,
        )
        self.probe(
            f"http://127.0.0.1:{PROBE_PORTS['auth_broker']}/readyz",
            expected_build=expected,
            expected_commit=source_sha,
        )

    def deploy(self, request: dict[str, Any]) -> dict[str, Any]:
        source_sha = request["source_sha"]
        current_main = self.fetch_main()
        if current_main != source_sha:
            raise DeploymentError(
                f"refusing stale deployment {source_sha}; current origin/main is {current_main}"
            )
        if self.read_json(self.bootstrapped_path) is None:
            raise DeploymentError("staging state is missing; run --bootstrap first")
        current_raw = self.read_json(self.current_path)
        unchanged = False
        if current_raw is not None:
            current = validate_stored_manifest(current_raw)
            unchanged = (
                not current["bootstrap"]
                and current["source_sha"] == source_sha
                and current["images"] == request["images"]
            )

        try:
            self.apply(request)
            browser_auth = self.issue_browser_acceptance_credentials()
        except Exception:
            try:
                self.record_runtime_diagnostics(source_sha)
            except Exception as diagnostics_error:
                log(f"failed to record deployment diagnostics: {diagnostics_error}")
            raise

        if not unchanged:
            self.atomic_json(self.history / f"{source_sha}.json", request)
            self.atomic_json(self.current_path, request)
        self.prune_releases()
        return {
            "ok": True,
            "action": "deploy",
            "source_sha": source_sha,
            "workflow_run_id": request["workflow_run_id"],
            "unchanged": unchanged,
            "browser_auth": browser_auth,
        }

    def handle(self, request: dict[str, Any]) -> dict[str, Any]:
        self.initialize_directories()
        lock_path = self.root / "deployment.lock"
        with lock_path.open("a+", encoding="utf-8") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if request.get("action") == "deploy":
                if request.get("schema_version") != SCHEMA_VERSION:
                    raise DeploymentError("unsupported deployment protocol")
                return self.deploy(validate_deploy_request(request))
            raise DeploymentError("unsupported deployment action")


def read_request() -> dict[str, Any]:
    raw = sys.stdin.buffer.read(65_537)
    if len(raw) > 65_536:
        raise DeploymentError("deployment request exceeds 64 KiB")
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        raise DeploymentError("deployment request is not valid JSON") from error
    if not isinstance(value, dict):
        raise DeploymentError("deployment request must be a JSON object")
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--bootstrap", action="store_true")
    mode.add_argument("--check", action="store_true")
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(os.environ.get("PATCHBAY_STAGING_ROOT", DEFAULT_ROOT)),
    )
    parser.add_argument(
        "--static-directory",
        type=Path,
        default=Path(
            os.environ.get("PATCHBAY_STAGING_STATIC", DEFAULT_STATIC_DIRECTORY)
        ),
    )
    arguments = parser.parse_args()
    deployment = StagingDeployment(arguments.root, arguments.static_directory)
    try:
        if arguments.bootstrap:
            receipt = deployment.bootstrap()
        elif arguments.check:
            receipt = deployment.check()
        else:
            receipt = deployment.handle(read_request())
        print(json.dumps(receipt, sort_keys=True), flush=True)
        return 0
    except (DeploymentError, OSError, subprocess.CalledProcessError) as error:
        log(f"staging deployment failed: {error}")
        print(json.dumps({"ok": False, "error": str(error)}, sort_keys=True), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
