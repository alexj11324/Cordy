import importlib.util
import io
import json
from pathlib import Path
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
        manifest["images"]["web"] = "ghcr.io/alexj11324/patchbay-web:latest"
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
                "image: ${PATCHBAY_IMAGE:?required}\nport: ${PORT:-8080}\n",
                encoding="utf-8",
            )
            self.assertEqual(
                production_deploy.compose_variables(compose),
                {"PATCHBAY_IMAGE", "PORT"},
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
            deployment.atomic_json(deployment.secrets / "auth-broker-env.json", {})

            self.assertEqual(deployment.check()["action"], "check")

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
            request.get_header("User-agent"), "PatchbayProductionDeploy/1"
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

    def test_deploy_requires_a_bootstrapped_rollback_target(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.fetch_main = mock.Mock(return_value="a" * 40)
            with self.assertRaisesRegex(production_deploy.DeploymentError, "--bootstrap"):
                deployment.deploy(self.manifest())

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

    def test_unchanged_deploy_does_not_rollback_after_verification_failure(self):
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

    def test_rollback_ignores_an_unchanged_redeployment_run(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            current = self.manifest()
            current["workflow_run_id"] = "111"
            previous = self.manifest()
            previous["source_sha"] = "b" * 40
            previous["workflow_run_id"] = "99"
            deployment.atomic_json(deployment.current_path, current)
            deployment.atomic_json(deployment.previous_path, previous)
            deployment.apply = mock.Mock()

            receipt = deployment.rollback("a" * 40, "222")

            self.assertTrue(receipt["unchanged"])
            self.assertEqual(receipt["source_sha"], "a" * 40)
            self.assertEqual(receipt["workflow_run_id"], "111")
            deployment.apply.assert_not_called()

    def test_legacy_rollback_protocol_cannot_bypass_run_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            with self.assertRaisesRegex(
                production_deploy.DeploymentError, "unsupported rollback protocol"
            ):
                deployment.handle(
                    {
                        "schema_version": 1,
                        "action": "rollback",
                        "failed_source_sha": "a" * 40,
                        "failed_workflow_run_id": "222",
                    }
                )

    def test_rollback_reverts_only_the_matching_deployment_run(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            current = self.manifest()
            current["workflow_run_id"] = "222"
            previous = self.manifest()
            previous["source_sha"] = "b" * 40
            previous["workflow_run_id"] = "111"
            deployment.atomic_json(deployment.current_path, current)
            deployment.atomic_json(deployment.previous_path, previous)
            deployment.apply = mock.Mock()
            deployment.prune_releases = mock.Mock()
            normalized_previous = production_deploy.validate_stored_manifest(previous)

            receipt = deployment.rollback("a" * 40, "222")

            self.assertFalse(receipt["unchanged"])
            self.assertEqual(receipt["source_sha"], "b" * 40)
            self.assertEqual(receipt["failed_workflow_run_id"], "222")
            deployment.apply.assert_called_once_with(normalized_previous)
            deployment.prune_releases.assert_called_once_with()

    def test_release_pruning_retains_only_current_and_rollback_worktrees(self):
        with tempfile.TemporaryDirectory() as directory:
            deployment = production_deploy.ProductionDeployment(Path(directory))
            deployment.initialize_directories()
            current = self.manifest()
            previous = self.manifest()
            previous["source_sha"] = "b" * 40
            deployment.atomic_json(deployment.current_path, current)
            deployment.atomic_json(deployment.previous_path, previous)
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
            self.assertEqual(len(removals), 1)
            self.assertEqual(Path(removals[0][-1]).name, "c" * 40)

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

    def test_bootstrap_recovery_uses_readiness_not_new_business_route_contract(self):
        urls = self.observed_apply_probes(bootstrap=True)
        self.assertIn("http://127.0.0.1:3110/login", urls)
        self.assertNotIn("http://127.0.0.1:3110/acme/issues", urls)
        self.assertNotIn("http://127.0.0.1:3110/acme/task-graph", urls)

    def test_gateway_readiness_does_not_treat_login_redirects_as_business_routes(self):
        urls = self.observed_apply_probes(bootstrap=False)
        self.assertNotIn("http://127.0.0.1:3110/acme/issues", urls)
        self.assertNotIn("http://127.0.0.1:3110/acme/task-graph", urls)


if __name__ == "__main__":
    unittest.main()
