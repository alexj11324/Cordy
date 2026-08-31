import importlib.util
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
            "repository": "alexj11324/Cordy",
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

    def test_normal_deploy_and_rollback_require_business_routes(self):
        urls = self.observed_apply_probes(bootstrap=False)
        self.assertIn("http://127.0.0.1:3110/acme/issues", urls)
        self.assertIn("http://127.0.0.1:3110/acme/task-graph", urls)


if __name__ == "__main__":
    unittest.main()
