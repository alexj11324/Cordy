#!/usr/bin/env python3
"""Restricted, auditable production deployment gateway.

The GitHub Actions SSH key is forced to this executable. The gateway accepts a
small JSON protocol on stdin, validates an exact current-main commit plus four
allow-listed GHCR digests, serializes deployments, and owns rollback. It never
executes a caller-provided command or shell fragment.
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
ROLLBACK_SCHEMA_VERSION = 2
REPOSITORY = "alexj11324/Cordy"  # legacy-brand-compat: current GitHub repository identity
REPOSITORY_URL = "https://github.com/alexj11324/Cordy.git"  # legacy-brand-compat
DEFAULT_ROOT = Path("/var/lib/patchbay-production")
DEFAULT_STATIC_DIRECTORY = Path("/usr/local/share/patchbay-production")
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
BOOTSTRAP_CONTAINERS = {
    "backend": "cordy632-backend-1",  # legacy-brand-compat: existing production project
    "web": "cordy632-frontend-1",  # legacy-brand-compat: existing production project
    "docs": "cordy-docs-1",  # legacy-brand-compat: existing production project
    "auth-broker": "patchbay-auth-broker-broker-1",
}
PRODUCTION_SMOKE_USER_EMAIL = "production-smoke@aspectlylabs.com"


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


def require_workflow_run_id(
    value: Any, label: str = "workflow_run_id"
) -> str:
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
        raise DeploymentError("deployment repository does not match the production allow-list")
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


def compose_variables(path: Path) -> set[str]:
    return set(COMPOSE_VARIABLE_RE.findall(path.read_text(encoding="utf-8")))


def parse_container_environment(entries: list[str]) -> dict[str, str]:
    result: dict[str, str] = {}
    for entry in entries:
        name, separator, value = entry.partition("=")
        if separator and name:
            result[name] = value
    return result


def select_environment(
    names: set[str], sources: list[dict[str, str]], explicit: dict[str, str]
) -> dict[str, str]:
    selected: dict[str, str] = {}
    for name in sorted(names):
        if name in explicit:
            selected[name] = explicit[name]
            continue
        for source in sources:
            if name in source:
                selected[name] = source[name]
                break
    selected.update(explicit)
    return selected


def select_bootstrap_image(name: str, configured: str, repo_digests: Any) -> str:
    if isinstance(repo_digests, list):
        for candidate in repo_digests:
            try:
                return validate_image_ref(name, candidate)
            except DeploymentError:
                continue
    return validate_image_ref(name, configured, immutable=False)


def clerk_api_request(
    secret_key: str,
    path: str,
    *,
    payload: dict[str, Any] | None = None,
) -> Any:
    if not secret_key.startswith("sk_"):
        raise DeploymentError("production Clerk secret is missing or invalid")
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    headers = {
        "Authorization": f"Bearer {secret_key}",
        "Accept": "application/json",
        "User-Agent": "PatchbayProductionDeploy/1",
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


class ProductionDeployment:
    def __init__(
        self,
        root: Path = DEFAULT_ROOT,
        static_directory: Path = DEFAULT_STATIC_DIRECTORY,
    ) -> None:
        self.root = root
        self.static_directory = static_directory
        self.repository = root / "repository.git"
        self.releases = root / "releases"
        self.history = root / "history"
        self.secrets = root / "secrets"
        self.current_path = root / "current.json"
        self.previous_path = root / "previous.json"

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
        else:
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
        for state_path in (self.current_path, self.previous_path):
            raw = self.read_json(state_path)
            if raw is not None:
                retained.add(validate_stored_manifest(raw)["source_sha"])

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

    def container_environment(self, container: str) -> dict[str, str]:
        raw = run(
            ["docker", "inspect", "--format", "{{json .Config.Env}}", container],
            capture=True,
        )
        entries = json.loads(raw)
        if not isinstance(entries, list):
            raise DeploymentError(f"container {container} has invalid environment metadata")
        return parse_container_environment(entries)

    def container_image(self, name: str, container: str) -> str:
        configured = run(
            ["docker", "inspect", "--format", "{{.Config.Image}}", container],
            capture=True,
        )
        image_id = run(
            ["docker", "inspect", "--format", "{{.Image}}", container],
            capture=True,
        )
        raw_digests = run(
            ["docker", "image", "inspect", "--format", "{{json .RepoDigests}}", image_id],
            capture=True,
        )
        return select_bootstrap_image(name, configured, json.loads(raw_digests))

    def bootstrap(self) -> dict[str, Any]:
        self.initialize_directories()
        source_sha = self.fetch_main()
        release = self.checkout(source_sha)
        product_compose = release / "docker-compose.selfhost.yml"
        broker_compose = release / "deploy/origin/auth-broker.compose.yml"

        backend_env = self.container_environment(BOOTSTRAP_CONTAINERS["backend"])
        web_env = self.container_environment(BOOTSTRAP_CONTAINERS["web"])
        postgres_env = self.container_environment("cordy632-postgres-1")  # legacy-brand-compat
        product_env = select_environment(
            compose_variables(product_compose)
            | compose_variables(self.static_directory / "production-product.override.yml"),
            [backend_env, web_env, postgres_env],
            {"BACKEND_PORT": "8210", "FRONTEND_PORT": "3110"},
        )
        broker_env = select_environment(
            compose_variables(broker_compose),
            [self.container_environment(BOOTSTRAP_CONTAINERS["auth-broker"])],
            {},
        )
        self.atomic_json(self.secrets / "product-env.json", product_env)
        self.atomic_json(self.secrets / "auth-broker-env.json", broker_env)

        manifest = {
            "schema_version": SCHEMA_VERSION,
            "action": "deploy",
            "repository": REPOSITORY,
            "source_sha": source_sha,
            "workflow_run_id": "bootstrap",
            "bootstrap": True,
            "images": {
                name: self.container_image(name, container)
                for name, container in BOOTSTRAP_CONTAINERS.items()
            },
        }
        validate_stored_manifest(manifest)
        self.atomic_json(self.current_path, manifest)
        self.prune_releases()
        return {"ok": True, "action": "bootstrap", "source_sha": source_sha}

    def check(self) -> dict[str, Any]:
        current_raw = self.read_json(self.current_path)
        if current_raw is None:
            raise DeploymentError("production state is missing; run --bootstrap first")
        current = validate_stored_manifest(current_raw)
        self.deployment_environment(current)
        for name in ("production-product.override.yml", "production-docs.compose.yml"):
            if not (self.static_directory / name).is_file():
                raise DeploymentError(f"installed deployment file is missing: {name}")
        return {"ok": True, "action": "check", "source_sha": current["source_sha"]}

    def deployment_environment(
        self, manifest: dict[str, Any]
    ) -> tuple[dict[str, str], dict[str, str]]:
        product = self.read_json(self.secrets / "product-env.json")
        broker = self.read_json(self.secrets / "auth-broker-env.json")
        if not isinstance(product, dict) or not isinstance(broker, dict):
            raise DeploymentError(
                "production environment snapshot is missing; run --bootstrap first"
            )
        if not all(
            isinstance(key, str) and isinstance(value, str) for key, value in product.items()
        ):
            raise DeploymentError("product environment snapshot is invalid")
        if not all(
            isinstance(key, str) and isinstance(value, str) for key, value in broker.items()
        ):
            raise DeploymentError("auth broker environment snapshot is invalid")

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
        product = self.read_json(self.secrets / "product-env.json")
        if not isinstance(product, dict):
            raise DeploymentError("production environment snapshot is missing")
        secret_key = product.get("CLERK_SECRET_KEY")
        if not isinstance(secret_key, str):
            raise DeploymentError("production Clerk secret is missing")

        query = urlencode(
            [("email_address", PRODUCTION_SMOKE_USER_EMAIL), ("limit", "2")]
        )
        users = clerk_users(clerk_api_request(secret_key, f"users?{query}"))
        exact = []
        for user in users:
            addresses = user.get("email_addresses")
            if not isinstance(addresses, list):
                continue
            if any(
                isinstance(address, dict)
                and address.get("email_address") == PRODUCTION_SMOKE_USER_EMAIL
                for address in addresses
            ):
                exact.append(user)
        if len(exact) != 1 or not isinstance(exact[0].get("id"), str):
            raise DeploymentError(
                "the dedicated production browser-acceptance Clerk user is missing or ambiguous"
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
        run(["docker", "compose", *arguments], env=env)

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
            "cordy632",  # legacy-brand-compat: preserves the production volumes
            "--project-directory",
            str(release),
            "-f",
            str(release / "docker-compose.selfhost.yml"),
            "-f",
            str(self.static_directory / "production-product.override.yml"),
        ]
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
                "cordy",  # legacy-brand-compat: preserves the production Compose project
                "-f",
                str(self.static_directory / "production-docs.compose.yml"),
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
                "patchbay-auth-broker",
                "-f",
                str(release / "deploy/origin/auth-broker.compose.yml"),
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

        is_bootstrap = manifest.get("bootstrap") is True
        expected = None if is_bootstrap else f"sha-{source_sha}"
        public_host_headers = {"Host": "patchbay.aspectlylabs.com", "X-Forwarded-Proto": "https"}
        self.probe(
            "http://127.0.0.1:8210/readyz",
            expected_build=expected,
            expected_commit=None if is_bootstrap else source_sha,
        )
        self.probe("http://127.0.0.1:8210/api/config")
        self.probe(
            "http://127.0.0.1:3110/login",
            expected_build=expected,
            expected_commit=None if is_bootstrap else source_sha,
            headers=public_host_headers,
        )
        self.probe(
            "http://127.0.0.1:4000/docs",
            expected_build=expected,
            expected_commit=None if is_bootstrap else source_sha,
        )
        self.probe(
            "http://127.0.0.1:43100/readyz",
            expected_build=expected,
            expected_commit=None if is_bootstrap else source_sha,
        )

    def deploy(self, request: dict[str, Any]) -> dict[str, Any]:
        source_sha = request["source_sha"]
        current_main = self.fetch_main()
        if current_main != source_sha:
            raise DeploymentError(
                f"refusing stale deployment {source_sha}; current origin/main is {current_main}"
            )
        current_raw = self.read_json(self.current_path)
        if current_raw is None:
            raise DeploymentError("production state is missing; run --bootstrap first")
        current = validate_stored_manifest(current_raw)
        unchanged = (
            not current["bootstrap"]
            and current["source_sha"] == source_sha
            and current["images"] == request["images"]
        )

        try:
            self.apply(request)
            browser_auth = self.issue_browser_acceptance_credentials()
        except Exception as deploy_error:
            if unchanged:
                log(
                    "deployment verification failed for an unchanged revision; "
                    "skipping rollback because this run made no state transition"
                )
                raise
            log(f"deployment failed; restoring {current['source_sha']}")
            try:
                self.apply(current)
            except Exception as rollback_error:
                raise DeploymentError(
                    f"deployment failed ({deploy_error}); automatic rollback also failed "
                    f"({rollback_error})"
                ) from rollback_error
            raise

        if not unchanged:
            self.atomic_json(self.history / f"{source_sha}.json", request)
            self.atomic_json(self.previous_path, current)
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

    def rollback(
        self, failed_source_sha: str, failed_workflow_run_id: str
    ) -> dict[str, Any]:
        failed_source_sha = require_sha(failed_source_sha, "failed_source_sha")
        failed_workflow_run_id = require_workflow_run_id(
            failed_workflow_run_id, "failed_workflow_run_id"
        )
        current_raw = self.read_json(self.current_path)
        if current_raw is None:
            raise DeploymentError("cannot roll back without a current deployment")
        current = validate_stored_manifest(current_raw)
        if (
            current["source_sha"] != failed_source_sha
            or current.get("workflow_run_id") != failed_workflow_run_id
        ):
            return {
                "ok": True,
                "action": "rollback",
                "source_sha": current["source_sha"],
                "workflow_run_id": current.get("workflow_run_id", ""),
                "unchanged": True,
            }
        previous_raw = self.read_json(self.previous_path)
        if previous_raw is None:
            raise DeploymentError("cannot roll back because no previous deployment is recorded")
        previous = validate_stored_manifest(previous_raw)
        self.apply(previous)
        self.atomic_json(self.history / f"failed-{failed_source_sha}.json", current)
        self.atomic_json(self.current_path, previous)
        self.previous_path.unlink(missing_ok=True)
        self.prune_releases()
        return {
            "ok": True,
            "action": "rollback",
            "source_sha": previous["source_sha"],
            "failed_source_sha": failed_source_sha,
            "failed_workflow_run_id": failed_workflow_run_id,
            "unchanged": False,
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
            if request.get("action") == "rollback":
                if request.get("schema_version") != ROLLBACK_SCHEMA_VERSION:
                    raise DeploymentError("unsupported rollback protocol")
                return self.rollback(
                    request.get("failed_source_sha"),
                    request.get("failed_workflow_run_id"),
                )
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
        default=Path(os.environ.get("PATCHBAY_PRODUCTION_ROOT", DEFAULT_ROOT)),
    )
    arguments = parser.parse_args()
    deployment = ProductionDeployment(arguments.root)
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
        log(f"production deployment failed: {error}")
        print(json.dumps({"ok": False, "error": str(error)}, sort_keys=True), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
