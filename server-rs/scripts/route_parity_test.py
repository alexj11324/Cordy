#!/usr/bin/env python3

import tempfile
import unittest
from pathlib import Path

import route_parity


class RouteParityTest(unittest.TestCase):
    def test_extracts_chained_methods_and_normalizes_routes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "routes.rs").write_text(
                """
                // .route("/ignored/comment", get(fake))
                const EXAMPLE: &str = ".route(\"/ignored/string\", get(fake))";
                Router::new()
                    .route("/api/issues/{issue_id}/", get(show).put(update))
                    .route(
                        "/api/issues/{id}/comments",
                        axum::routing::post(create).delete(remove),
                    );
                """,
                encoding="utf-8",
            )

            self.assertEqual(
                route_parity.extract_rust_routes(root),
                {
                    ("GET", "/api/issues/{}"),
                    ("PUT", "/api/issues/{}"),
                    ("POST", "/api/issues/{}/comments"),
                    ("DELETE", "/api/issues/{}/comments"),
                },
            )

    def test_rejects_duplicate_contract_entries_after_normalization(self):
        with tempfile.TemporaryDirectory() as directory:
            contract = Path(directory) / "routes.tsv"
            contract.write_text(
                "GET\t/api/issues/{id}\nGET\t/api/issues/{issue_id}/\n",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "duplicate route"):
                route_parity.load_contract(contract)

    def test_rejects_an_incomplete_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            contract = Path(directory) / "routes.tsv"
            contract.write_text("GET\t/health\n", encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "expected 424 routes"):
                route_parity.load_contract(contract)

    def test_normalizes_bare_and_named_wildcards_to_the_same_contract(self):
        self.assertEqual(route_parity.normalize_route("/uploads/*"), "/uploads/{*}")
        self.assertEqual(
            route_parity.normalize_route("/uploads/{*path}"), "/uploads/{*}"
        )


if __name__ == "__main__":
    unittest.main()
