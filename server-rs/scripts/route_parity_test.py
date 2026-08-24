#!/usr/bin/env python3

import tempfile
import unittest
from pathlib import Path

import route_parity


class RouteParityTest(unittest.TestCase):
    def test_extracts_chained_methods_and_normalizes_routes(self):
        source = """
                // .route("/ignored/comment", get(fake))
                const EXAMPLE: &str = ".route(\"/ignored/string\", get(fake))";
                Router::new()
                    .route("/api/issues/{issue_id}/", get(show).put(update))
                    .route(
                        "/api/issues/{id}/comments",
                        axum::routing::post(create).delete(remove),
                    );
                """

        self.assertEqual(
            route_parity.extract_routes(source),
            {
                ("GET", "/api/issues/{}"),
                ("PUT", "/api/issues/{}"),
                ("POST", "/api/issues/{}/comments"),
                ("DELETE", "/api/issues/{}/comments"),
            },
        )

    def test_extracts_only_routes_reachable_from_build_router(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text(
                """
                mod mounted;
                mod unmounted;

                pub fn build_router() -> Router {
                    build_router_from_state()
                }

                fn build_router_from_state() -> Router {
                    let discarded = unmounted::router();
                    let state = make_state(unmounted::router());
                    let mounted_routes = mounted::router().route_layer(layer());
                    #[cfg(test)] {
                        let _test = Router::new().route("/body-test-only", get(handler));
                    }
                    Router::new()
                        .merge(mounted_routes)
                        .route("/direct", get(direct))
                        .with_state(state)
                }

                #[cfg(test)]
                mod tests {
                    fn router() -> Router {
                        Router::new().route("/test-only", get(handler))
                    }
                }
                """,
                encoding="utf-8",
            )
            (root / "mounted.rs").write_text(
                """
                #[cfg(not(test))]
                pub fn router() -> Router {
                    Router::new().route("/mounted", get(handler))
                }

                #[cfg(test)]
                pub fn router() -> Router {
                    Router::new().route("/module-test-only", get(handler))
                }

                fn unused_router() -> Router {
                    Router::new().route("/unused-helper", get(handler))
                }
                """,
                encoding="utf-8",
            )
            (root / "unmounted.rs").write_text(
                """
                pub fn router() -> Router {
                    Router::new().route("/unmounted", get(handler))
                }
                """,
                encoding="utf-8",
            )

            self.assertEqual(
                route_parity.extract_rust_routes(root),
                {("GET", "/direct"), ("GET", "/mounted")},
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

    def test_normalizes_chi_and_axum_wildcards_to_the_same_contract(self):
        self.assertEqual(route_parity.normalize_route("/uploads/*"), "/uploads/{*}")
        self.assertEqual(
            route_parity.normalize_route("/uploads/{*path}"), "/uploads/{*}"
        )


if __name__ == "__main__":
    unittest.main()
