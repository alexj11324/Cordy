import base64
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("production_deploy.py")
SPEC = importlib.util.spec_from_file_location("production_deploy", MODULE_PATH)
assert SPEC and SPEC.loader
production_deploy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(production_deploy)


class ProductionDeployContractTests(unittest.TestCase):
    def manifest(self):
        source_sha = "a" * 40
        images = {
            name: f"{repository}@sha256:{'b' * 64}"
            for name, repository in production_deploy.EXPECTED_IMAGE_REPOSITORIES.items()
        }
        return {
            "schema_version": 1,
            "action": "deploy",
            "repository": "alexj11324/Cordy",  # legacy-brand-compat: live repository identity
            "source_sha": source_sha,
            "workflow_run_id": "123",
            "images": images,
        }

    def test_accepts_only_the_complete_immutable_image_set(self):
        normalized = production_deploy.validate_deploy_request(self.manifest())
        self.assertEqual(set(normalized["images"]), {"backend", "web", "docs", "auth-broker"})
        self.assertFalse(normalized["bootstrap"])

    def test_rejects_mutable_tags_from_the_network_request(self):
        manifest = self.manifest()
        manifest["images"]["web"] = "ghcr.io/alexj11324/orvilo-web:latest"
        with self.assertRaisesRegex(production_deploy.DeploymentError, "sha256 digest"):
            production_deploy.validate_deploy_request(manifest)

    def test_rejects_an_incomplete_image_set(self):
        manifest = self.manifest()
        del manifest["images"]["docs"]
        with self.assertRaisesRegex(production_deploy.DeploymentError, "exactly"):
            production_deploy.validate_deploy_request(manifest)

    def test_rejects_a_missing_or_invalid_workflow_run_id(self):
        for value in (None, "", "0", "local", "-1"):
            manifest = self.manifest()
            manifest["workflow_run_id"] = value
            with self.subTest(value=value), self.assertRaisesRegex(
                production_deploy.DeploymentError, "GitHub Actions run ID"
            ):
                production_deploy.validate_deploy_request(manifest)

    def test_extracts_compose_variables_without_values(self):
        with tempfile.TemporaryDirectory() as directory:
            compose = Path(directory) / "compose.yml"
            compose.write_text(
                "image: ${ORVILO_IMAGE:?required}\nport: ${PORT:-8080}\n",
                encoding="utf-8",
            )
            self.assertEqual(
                production_deploy.compose_variables(compose),
                {"ORVILO_IMAGE", "PORT"},
            )

    def test_environment_snapshot_prefers_explicit_safe_ports(self):
        selected = production_deploy.select_environment(
            {"PORT", "BACKEND_PORT", "JWT_SECRET"},
            [{"PORT": "8080", "JWT_SECRET": "secret"}],
            {"BACKEND_PORT": "8210"},
        )
        self.assertEqual(
            selected,
            {"BACKEND_PORT": "8210", "JWT_SECRET": "secret", "PORT": "8080"},
        )

    def test_bootstrap_prefers_the_allowlisted_immutable_repo_digest(self):
        repository = production_deploy.EXPECTED_IMAGE_REPOSITORIES["web"]
        digest_ref = f"{repository}@sha256:{'c' * 64}"
        self.assertEqual(
            production_deploy.select_bootstrap_image(
                "web",
                f"{repository}:old-tag",
                ["docker.io/example/other@sha256:" + "d" * 64, digest_ref],
            ),
            digest_ref,
        )

    def test_bootstrap_falls_back_to_an_allowlisted_configured_tag(self):
        repository = production_deploy.EXPECTED_IMAGE_REPOSITORIES["docs"]
        configured = f"{repository}:old-tag"
        self.assertEqual(
            production_deploy.select_bootstrap_image("docs", configured, []),
            configured,
        )

    def test_bootstrap_accepts_the_current_legacy_image_only_for_migration(self):
        repository = production_deploy.LEGACY_BOOTSTRAP_IMAGE_REPOSITORIES["backend"]
        digest_ref = f"{repository}@sha256:{'d' * 64}"
        self.assertEqual(
            production_deploy.select_bootstrap_image(
                "backend",
                f"{repository}:old-tag",
                [digest_ref],
            ),
            digest_ref,
        )
        manifest = self.manifest()
        manifest["bootstrap"] = True
        manifest["images"]["backend"] = digest_ref
        self.assertEqual(
            production_deploy.validate_stored_manifest(manifest)["images"]["backend"],
            digest_ref,
        )

    def test_network_deploy_rejects_legacy_image_repositories(self):
        manifest = self.manifest()
        legacy = production_deploy.LEGACY_BOOTSTRAP_IMAGE_REPOSITORIES["web"]
        manifest["images"]["web"] = f"{legacy}@sha256:{'d' * 64}"
        with self.assertRaisesRegex(
            production_deploy.DeploymentError, "allow-listed repository"
        ):
            production_deploy.validate_deploy_request(manifest)

    def test_check_validates_existing_state_without_rebootstrapping(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "state"
            static = Path(directory) / "static"
            static.mkdir()
            for name in ("production-product.override.yml", "production-docs.compose.yml"):
                (static / name).write_text("services: {}\n", encoding="utf-8")
            deployment = production_deploy.ProductionDeployment(root, static)
            deployment.initialize_directories()
            deployment.atomic_json(deployment.current_path, self.manifest())
            deployment.atomic_json(deployment.secrets / "product-env.json", {})
            deployment.atomic_json(
                deployment.secrets / "auth-broker-env.json",
                {"CLERK_PUBLISHABLE_KEY": "pk_live_fixture"},
            )

            with mock.patch.object(
                production_deploy, "run", return_value=base64.b64encode(b"k" * 32).decode()
            ):
                self.assertEqual(deployment.check()["action"], "check")

    def test_web_runtime_reuses_the_broker_publishable_key(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            deployment.atomic_json(
                deployment.secrets / "product-env.json",
                {"CLERK_SECRET_KEY": "sk_live_fixture"},
            )
            deployment.atomic_json(
                deployment.secrets / "auth-broker-env.json",
                {"CLERK_PUBLISHABLE_KEY": " pk_live_fixture "},
            )

            with mock.patch.object(
                production_deploy, "run", return_value=base64.b64encode(b"k" * 32).decode()
            ):
                product_env, broker_env = deployment.deployment_environment(self.manifest())

            self.assertEqual(product_env["ORVILO_CLERK_PUBLISHABLE_KEY"], "pk_live_fixture")
            self.assertEqual(broker_env["CLERK_PUBLISHABLE_KEY"], " pk_live_fixture ")

    def test_hosted_messaging_keys_override_empty_snapshots_without_persisting(self):
        expected = {
            "ORVILO_LARK_SECRET_KEY": "orvilo-lark-secret-key",
            "ORVILO_DINGTALK_SECRET_KEY": "orvilo-dingtalk-secret-key",
            "ORVILO_WECOM_SECRET_KEY": "orvilo-wecom-secret-key",
            "ORVILO_TELEGRAM_SECRET_KEY": "orvilo-telegram-secret-key",
            "ORVILO_WEIXIN_SECRET_KEY": "orvilo-weixin-secret-key",
        }
        values = {
            secret: base64.b64encode(bytes([index]) * 32).decode()
            for index, secret in enumerate(expected.values(), 1)
        }
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            product = {
                "ORVILO_MESSAGING_MODE": "server_configured",
                "ORVILO_LARK_SECRET_KEY": "",
                "ORVILO_WECOM_SECRET_KEY": "",
                "ORVILO_SLACK_SECRET_KEY": "existing-slack-key",
            }
            snapshot = deployment.secrets / "product-env.json"
            deployment.atomic_json(snapshot, product)
            deployment.atomic_json(
                deployment.secrets / "auth-broker-env.json",
                {"CLERK_PUBLISHABLE_KEY": "pk_live_fixture"},
            )
            before = snapshot.read_bytes()

            def read_secret(arguments, *, capture):
                self.assertTrue(capture)
                self.assertEqual(arguments[:4], ["gcloud", "secrets", "versions", "access"])
                self.assertEqual(arguments[4:7], ["1", "--project", "general-secrets-store"])
                self.assertEqual(arguments[7], "--secret")
                return values[arguments[8]]

            with mock.patch.object(production_deploy, "run", side_effect=read_secret):
                product_env, _ = deployment.deployment_environment(self.manifest())

            for name, secret in expected.items():
                self.assertEqual(product_env.get(name), values[secret])
            self.assertEqual(product_env["ORVILO_SLACK_SECRET_KEY"], "existing-slack-key")
            self.assertEqual(snapshot.read_bytes(), before)

    def test_messaging_disabled_does_not_require_gsm(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            deployment.atomic_json(
                deployment.secrets / "product-env.json",
                {"ORVILO_MESSAGING_MODE": " disabled ", "ORVILO_SLACK_SECRET_KEY": "existing"},
            )
            deployment.atomic_json(
                deployment.secrets / "auth-broker-env.json",
                {"CLERK_PUBLISHABLE_KEY": "pk_live_fixture"},
            )
            with mock.patch.object(production_deploy, "run") as run:
                product_env, _ = deployment.deployment_environment(self.manifest())
            run.assert_not_called()
            self.assertEqual(product_env["ORVILO_SLACK_SECRET_KEY"], "existing")

    def test_invalid_messaging_key_stops_before_container_changes(self):
        for invalid in ("not-a-base64-key", base64.b64encode(b"short").decode()):
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory() as directory:
                deployment = production_deploy.ProductionDeployment(Path(directory))
                deployment.initialize_directories()
                deployment.atomic_json(deployment.secrets / "product-env.json", {})
                deployment.atomic_json(
                    deployment.secrets / "auth-broker-env.json",
                    {"CLERK_PUBLISHABLE_KEY": "pk_live_fixture"},
                )
                deployment.checkout = mock.Mock(return_value=Path(directory) / "release")
                deployment.compose = mock.Mock()
                deployment.probe = mock.Mock()
                with mock.patch.object(production_deploy, "run", return_value=invalid) as run:
                    with self.assertRaises(production_deploy.DeploymentError) as caught:
                        deployment.apply(self.manifest())
                self.assertNotIn(invalid, str(caught.exception))
                self.assertTrue(all(call.args[0][0] == "gcloud" for call in run.call_args_list))
                deployment.compose.assert_not_called()

    def test_unavailable_messaging_key_has_no_secret_output_or_container_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            deployment.atomic_json(deployment.secrets / "product-env.json", {})
            deployment.atomic_json(
                deployment.secrets / "auth-broker-env.json",
                {"CLERK_PUBLISHABLE_KEY": "pk_live_fixture"},
            )
            deployment.checkout = mock.Mock(return_value=Path(directory) / "release")
            deployment.compose = mock.Mock()
            deployment.probe = mock.Mock()
            error = subprocess.CalledProcessError(
                1, ["gcloud"], output="secret-output", stderr="secret-error"
            )
            with mock.patch.object(production_deploy, "run", side_effect=error):
                with self.assertRaises(production_deploy.DeploymentError) as caught:
                    deployment.apply(self.manifest())
            self.assertNotIn("secret-output", str(caught.exception))
            self.assertNotIn("secret-error", str(caught.exception))
            deployment.compose.assert_not_called()

    def test_receipt_payload_is_json_serializable(self):
        self.assertEqual(json.loads(json.dumps(self.manifest()))["schema_version"], 1)

    def test_clerk_user_list_accepts_raw_and_paginated_responses(self):
        user = {"id": "user_smoke"}
        self.assertEqual(production_deploy.clerk_users([user]), [user])
        self.assertEqual(production_deploy.clerk_users({"data": [user]}), [user])
        with self.assertRaisesRegex(production_deploy.DeploymentError, "invalid user-list"):
            production_deploy.clerk_users({"data": "invalid"})

    def test_clerk_api_request_identifies_the_deployment_client(self):
        with mock.patch.object(
            production_deploy, "urlopen", return_value=io.BytesIO(b"{}")
        ) as urlopen:
            self.assertEqual(
                production_deploy.clerk_api_request("sk_live_fixture", "users"),
                {},
            )

        request = urlopen.call_args.args[0]
        self.assertEqual(
            request.get_header("User-agent"), "OrviloProductionDeploy/1"
        )

    def test_browser_credentials_are_short_lived_and_bound_to_the_smoke_user(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            deployment.atomic_json(
                deployment.secrets / "product-env.json",
                {"CLERK_SECRET_KEY": "sk_live_fixture"},
            )
            calls = []

            def fake_clerk_request(_secret, path, *, payload=None):
                calls.append((path, payload))
                if path.startswith("users?"):
                    return [
                        {
                            "id": "user_smoke",
                            "email_addresses": [
                                {
                                    "email_address": production_deploy.PRODUCTION_SMOKE_USER_EMAIL
                                }
                            ],
                        }
                    ]
                if path == "sign_in_tokens":
                    return {"token": "sign-in-ticket"}
                if path == "testing_tokens":
                    return {"token": "testing-token"}
                raise AssertionError(path)

            with mock.patch.object(
                production_deploy, "clerk_api_request", side_effect=fake_clerk_request
            ):
                credentials = deployment.issue_browser_acceptance_credentials()

            self.assertEqual(
                credentials,
                {
                    "sign_in_ticket": "sign-in-ticket",
                    "testing_token": "testing-token",
                },
            )
            self.assertIn(
                (
                    "sign_in_tokens",
                    {"user_id": "user_smoke", "expires_in_seconds": 300},
                ),
                calls,
            )

    def test_deploy_requires_bootstrapped_production_state(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.fetch_main = mock.Mock(return_value="a" * 40)
            with self.assertRaisesRegex(production_deploy.DeploymentError, "--bootstrap"):
                deployment.deploy(self.manifest())

    def test_failed_deploy_records_diagnostics_without_restoring_old_images(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            current = self.manifest()
            current["source_sha"] = "b" * 40
            deployment.atomic_json(deployment.current_path, current)
            deployment.fetch_main = mock.Mock(return_value="a" * 40)
            deployment.apply = mock.Mock(
                side_effect=production_deploy.DeploymentError("candidate failed")
            )
            deployment.record_runtime_diagnostics = mock.Mock()

            with self.assertRaisesRegex(
                production_deploy.DeploymentError, "candidate failed"
            ):
                deployment.deploy(self.manifest())

            deployment.record_runtime_diagnostics.assert_called_once_with("a" * 40)
            deployment.apply.assert_called_once_with(self.manifest())

    def test_runtime_diagnostics_timeout_cannot_delay_failure_reporting(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            timeout = production_deploy.subprocess.TimeoutExpired(
                ["docker", "inspect"],
                timeout=5,
                output="partial output",
            )

            with mock.patch.object(
                production_deploy.subprocess,
                "run",
                side_effect=timeout,
            ) as run:
                deployment.record_runtime_diagnostics("a" * 40)

            self.assertEqual(
                run.call_count,
                len(production_deploy.BOOTSTRAP_CONTAINERS) * 2,
            )
            self.assertTrue(
                all(call.kwargs["timeout"] == 5 for call in run.call_args_list)
            )
            diagnostics = json.loads(
                (deployment.history / f"failed-runtime-{'a' * 40}.json").read_text()
            )
            for entry in diagnostics["containers"].values():
                self.assertIn("[diagnostic command timed out]", entry["state"])
                self.assertIsNone(entry["state_exit_code"])
                self.assertIn("[diagnostic command timed out]", entry["logs"])
                self.assertIsNone(entry["logs_exit_code"])

    def test_deploy_receipt_contains_browser_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            previous = self.manifest()
            previous["source_sha"] = "b" * 40
            deployment.atomic_json(deployment.current_path, previous)
            deployment.fetch_main = mock.Mock(return_value="a" * 40)
            deployment.apply = mock.Mock()
            deployment.issue_browser_acceptance_credentials = mock.Mock(
                return_value={
                    "sign_in_ticket": "sign-in-ticket",
                    "testing_token": "testing-token",
                }
            )
            deployment.prune_releases = mock.Mock()

            receipt = deployment.deploy(self.manifest())

            self.assertEqual(receipt["browser_auth"]["sign_in_ticket"], "sign-in-ticket")
            self.assertEqual(receipt["workflow_run_id"], "123")
            self.assertFalse(receipt["unchanged"])
            deployment.prune_releases.assert_called_once_with()

    def test_unchanged_deploy_does_not_reapply_old_images_after_verification_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            current = self.manifest()
            deployment.atomic_json(deployment.current_path, current)
            deployment.fetch_main = mock.Mock(return_value=current["source_sha"])
            deployment.apply = mock.Mock()
            deployment.issue_browser_acceptance_credentials = mock.Mock(
                side_effect=production_deploy.DeploymentError("temporary verifier failure")
            )

            with self.assertRaisesRegex(
                production_deploy.DeploymentError, "temporary verifier failure"
            ):
                deployment.deploy(current)

            deployment.apply.assert_called_once_with(current)

    def test_rejects_rollback_actions(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            with self.assertRaisesRegex(
                production_deploy.DeploymentError, "unsupported deployment action"
            ):
                deployment.handle(
                    {
                        "schema_version": 2,
                        "action": "rollback",
                        "failed_source_sha": "a" * 40,
                        "failed_workflow_run_id": "222",
                    }
                )

    def test_release_pruning_retains_only_the_current_worktree(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            current = self.manifest()
            deployment.atomic_json(deployment.current_path, current)
            for sha in ("a" * 40, "b" * 40, "c" * 40):
                (deployment.releases / sha).mkdir()

            observed = []

            def fake_run(arguments, *, env=None, capture=False):
                observed.append(arguments)
                if capture and arguments[1] == "-C":
                    return Path(arguments[2]).name
                return ""

            with mock.patch.object(production_deploy, "run", side_effect=fake_run):
                deployment.prune_releases()

            removals = [
                arguments
                for arguments in observed
                if "worktree" in arguments and "remove" in arguments
            ]
            self.assertEqual(len(removals), 2)
            self.assertEqual(
                {Path(arguments[-1]).name for arguments in removals},
                {"b" * 40, "c" * 40},
            )

    def observed_apply_probes(self, *, bootstrap):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.checkout = mock.Mock(return_value=Path(directory) / "release")
            deployment.deployment_environment = mock.Mock(return_value=({}, {}))
            deployment.compose = mock.Mock()
            deployment.probe = mock.Mock()
            manifest = self.manifest()
            manifest["bootstrap"] = bootstrap
            with mock.patch.object(production_deploy, "run"):
                deployment.apply(manifest)
            return [call.args[0] for call in deployment.probe.call_args_list]

    def test_apply_uses_overlays_from_the_verified_release(self):
        with tempfile.TemporaryDirectory() as directory:
            release = Path(directory) / "release"
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.checkout = mock.Mock(return_value=release)
            deployment.deployment_environment = mock.Mock(return_value=({}, {}))
            deployment.compose = mock.Mock()
            deployment.probe = mock.Mock()

            with mock.patch.object(production_deploy, "run"):
                deployment.apply(self.manifest())

            compose_arguments = [
                call.args[0] for call in deployment.compose.call_args_list
            ]
            self.assertTrue(
                all(
                    str(release / "deploy/origin/production-product.override.yml")
                    in arguments
                    for arguments in compose_arguments[:2]
                )
            )
            self.assertIn(
                str(release / "deploy/origin/production-docs.compose.yml"),
                compose_arguments[2],
            )
            self.assertTrue(
                all(
                    str(deployment.static_directory) not in " ".join(arguments)
                    for arguments in compose_arguments
                )
            )

    def test_bootstrap_recovery_uses_readiness_not_new_business_route_contract(self):
        urls = self.observed_apply_probes(bootstrap=True)
        self.assertIn("http://127.0.0.1:3110/login", urls)
        self.assertNotIn("http://127.0.0.1:3110/acme/issues", urls)
        self.assertNotIn("http://127.0.0.1:3110/acme/task-graph", urls)

    def test_gateway_readiness_does_not_treat_login_redirects_as_business_routes(self):
        urls = self.observed_apply_probes(bootstrap=False)
        self.assertNotIn("http://127.0.0.1:3110/acme/issues", urls)
        self.assertNotIn("http://127.0.0.1:3110/acme/task-graph", urls)

    def test_probe_retries_a_connection_reset_during_container_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            response = mock.MagicMock()
            response.status = 200
            response.headers = {}
            connection = mock.MagicMock()
            connection.__enter__.return_value = response

            with (
                mock.patch.object(
                    production_deploy,
                    "urlopen",
                    side_effect=[ConnectionResetError("peer reset"), connection],
                ) as urlopen,
                mock.patch.object(production_deploy.time, "sleep") as sleep,
            ):
                deployment.probe("http://127.0.0.1:8210/readyz")

            self.assertEqual(urlopen.call_count, 2)
            sleep.assert_called_once_with(5)


if __name__ == "__main__":
    unittest.main()
