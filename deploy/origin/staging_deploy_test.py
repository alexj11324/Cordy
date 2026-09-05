import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


MODULE_PATH = Path(__file__).with_name("staging_deploy.py")
SPEC = importlib.util.spec_from_file_location("staging_deploy", MODULE_PATH)
assert SPEC and SPEC.loader
staging_deploy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(staging_deploy)


def valid_product_env():
    return {
        "PATCHBAY_PUBLIC_URL": "https://api.staging.aspectlylabs.com",
        "PATCHBAY_APP_URL": "https://staging.aspectlylabs.com",
        "FRONTEND_ORIGIN": "https://staging.aspectlylabs.com",
        "BACKEND_PORT": "8211",
        "FRONTEND_PORT": "3111",
        "COOKIE_DOMAIN": ".staging.aspectlylabs.com",
        "CLERK_SECRET_KEY": "sk_test_staging",
        "JWT_SECRET": "staging-jwt",
    }


def valid_broker_env():
    return {
        "PATCHBAY_API_ORIGIN": "https://api.staging.aspectlylabs.com",
        "PATCHBAY_AUTH_BROKER_ORIGIN": "https://accounts.staging.aspectlylabs.com",
        "CLERK_PUBLISHABLE_KEY": "pk_test_staging",
        "PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN": "a" * 64,
        "PATCHBAY_ORIGIN_AUTH_TOKEN": "b" * 64,
    }


class StagingDeployIsolationTests(unittest.TestCase):
    def manifest(self):
        source_sha = "a" * 40
        images = {
            name: f"{repository}@sha256:{'b' * 64}"
            for name, repository in staging_deploy.EXPECTED_IMAGE_REPOSITORIES.items()
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
        normalized = staging_deploy.validate_deploy_request(self.manifest())
        self.assertEqual(set(normalized["images"]), {"backend", "web", "docs", "auth-broker"})

    def test_rejects_production_compose_projects(self):
        for name in ("cordy632", "cordy", "patchbay-auth-broker"):
            with self.subTest(name=name):
                with self.assertRaisesRegex(staging_deploy.DeploymentError, "production Compose"):
                    staging_deploy.assert_isolated_project(name)

    def test_accepts_staging_compose_projects(self):
        self.assertEqual(
            staging_deploy.assert_isolated_project("patchbay-staging"),
            "patchbay-staging",
        )

    def test_rejects_production_root(self):
        with tempfile.TemporaryDirectory() as directory:
            production = Path(directory) / "patchbay-production"
            production.mkdir()
            with self.assertRaisesRegex(staging_deploy.DeploymentError, "production path"):
                staging_deploy.assert_isolated_path(
                    production,
                    forbidden=production,
                    label="staging root",
                )

    def test_rejects_production_product_urls(self):
        values = valid_product_env()
        values["PATCHBAY_PUBLIC_URL"] = "https://api.aspectlylabs.com"
        with self.assertRaisesRegex(staging_deploy.DeploymentError, "production marker"):
            staging_deploy.reject_production_markers(values, label="staging product")

    def test_rejects_mismatched_staging_urls(self):
        values = valid_product_env()
        values["PATCHBAY_APP_URL"] = "https://patchbay-app.copilothub.ai"
        with self.assertRaisesRegex(staging_deploy.DeploymentError, "must be"):
            staging_deploy.require_exact(
                values, staging_deploy.STAGING_URLS, label="staging product"
            )

    def test_load_secrets_requires_staging_cookie_domain(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "staging"
            static_directory = Path(directory) / "static"
            static_directory.mkdir()
            for name in (
                "staging-product.override.yml",
                "staging-docs.compose.yml",
                "staging-auth-broker.compose.yml",
            ):
                (static_directory / name).write_text("name: patchbay-staging\n", encoding="utf-8")
            deployment = staging_deploy.StagingDeployment(root, static_directory)
            deployment.initialize_directories()
            product = valid_product_env()
            product["COOKIE_DOMAIN"] = ".aspectlylabs.com"
            (deployment.secrets / "product-env.json").write_text(
                json.dumps(product), encoding="utf-8"
            )
            (deployment.secrets / "auth-broker-env.json").write_text(
                json.dumps(valid_broker_env()), encoding="utf-8"
            )
            with self.assertRaisesRegex(staging_deploy.DeploymentError, "COOKIE_DOMAIN"):
                deployment.load_secrets()

    def test_load_secrets_accepts_isolated_staging_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "staging"
            static_directory = Path(directory) / "static"
            static_directory.mkdir()
            deployment = staging_deploy.StagingDeployment(root, static_directory)
            deployment.initialize_directories()
            (deployment.secrets / "product-env.json").write_text(
                json.dumps(valid_product_env()), encoding="utf-8"
            )
            (deployment.secrets / "auth-broker-env.json").write_text(
                json.dumps(valid_broker_env()), encoding="utf-8"
            )
            product, broker = deployment.load_secrets()
            self.assertEqual(product["BACKEND_PORT"], "8211")
            self.assertEqual(
                broker["PATCHBAY_AUTH_BROKER_ORIGIN"],
                "https://accounts.staging.aspectlylabs.com",
            )
            self.assertEqual(product["PATCHBAY_CLERK_PUBLISHABLE_KEY"], "pk_test_staging")

    def test_ports_and_projects_do_not_overlap_production(self):
        self.assertNotEqual(
            staging_deploy.STAGING_PORTS["BACKEND_PORT"],
            "8210",
        )
        self.assertNotEqual(
            staging_deploy.STAGING_PORTS["FRONTEND_PORT"],
            "3110",
        )
        self.assertTrue(
            staging_deploy.FORBIDDEN_COMPOSE_PROJECTS.isdisjoint(
                {
                    staging_deploy.PRODUCT_COMPOSE_PROJECT,
                    staging_deploy.DOCS_COMPOSE_PROJECT,
                    staging_deploy.AUTH_BROKER_COMPOSE_PROJECT,
                }
            )
        )
        self.assertNotEqual(staging_deploy.DEFAULT_ROOT, staging_deploy.PRODUCTION_ROOT)


if __name__ == "__main__":
    unittest.main()
