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

    def test_extracts_only_outer_method_router_verbs(self):
        source = """
                Router::new()
                    .route("/layered", post(handler).layer(state.get()))
                    .route("/inline", get(|| async { client.delete().await }))
                    .route(
                        "/chained",
                        axum::routing::post(create)
                            .delete(remove)
                            .layer(state.get()),
                    );
                """

        self.assertEqual(
            route_parity.extract_routes(source),
            {
                ("POST", "/layered"),
                ("GET", "/inline"),
                ("POST", "/chained"),
                ("DELETE", "/chained"),
            },
        )

    def test_rejects_unparsed_method_router_expressions(self):
        source = 'Router::new().route("/items", methods);'
        with self.assertRaisesRegex(ValueError, "unsupported MethodRouter"):
            route_parity.extract_mounted_routes(source)

    def test_rejects_nonliteral_mounted_route_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text(
                """
                const ISSUE_PATH: &str = "/issues";

                pub fn build_router() -> Router {
                    Router::new().route(ISSUE_PATH, get(handler))
                }
                """,
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "nonliteral route path"):
                route_parity.extract_rust_routes(root)

    def test_rejects_compound_test_cfg_predicates(self):
        for predicate in (
            "all(test, unix)",
            'any(test, feature = "test-utils")',
            'feature = "optional-router"',
        ):
            with self.subTest(predicate=predicate):
                source = f"""
                        #[cfg({predicate})]
                        fn guarded() -> Router {{
                            Router::new().route("/guarded", get(handler))
                        }}
                        """
                with self.assertRaisesRegex(ValueError, "unsupported cfg predicate"):
                    route_parity.production_source(source)

        filtered = route_parity.production_source(
            """
            #[cfg(test)]
            mod tests {
                #[cfg(all(test, unix))]
                fn nested_test_only() {}
            }

            #[cfg(not(test))]
            fn production() {}
            """
        )
        self.assertNotIn("nested_test_only", filtered)
        self.assertIn("fn production", filtered)

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

    def test_accepts_match_arms_that_only_layer_the_same_router(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text(
                """
                pub fn build_router() -> Router {
                    let app = Router::new().route("/mounted", get(handler));
                    match metrics {
                        Some(metrics) => app.layer(with_metrics(metrics)),
                        None => app,
                    }
                }
                """,
                encoding="utf-8",
            )

            self.assertEqual(
                route_parity.extract_rust_routes(root),
                {("GET", "/mounted")},
            )

    def test_resolves_router_modules_relative_to_the_calling_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            nested = root / "nested"
            nested.mkdir()
            (root / "lib.rs").write_text(
                """
                pub fn build_router() -> Router {
                    nested::router()
                }
                """,
                encoding="utf-8",
            )
            (nested / "mod.rs").write_text(
                """
                pub fn router() -> Router {
                    Router::new()
                        .merge(self::foo::router())
                        .merge(child::router())
                }
                """,
                encoding="utf-8",
            )
            for path, route in (
                (root / "foo.rs", "/wrong-root-foo"),
                (root / "child.rs", "/wrong-root-child"),
                (nested / "foo.rs", "/nested-foo"),
                (nested / "child.rs", "/nested-child"),
            ):
                path.write_text(
                    f"""
                    pub fn router() -> Router {{
                        Router::new().route("{route}", get(handler))
                    }}
                    """,
                    encoding="utf-8",
                )

            self.assertEqual(
                route_parity.extract_rust_routes(root),
                {("GET", "/nested-foo"), ("GET", "/nested-child")},
            )

            deep = nested / "deep.rs"
            deep.touch()
            (nested / "sibling.rs").touch()
            (root / "sibling.rs").touch()
            self.assertEqual(
                route_parity.module_source(root, deep, ["super", "sibling"]),
                nested / "sibling.rs",
            )
            self.assertEqual(
                route_parity.module_source(root, deep, ["crate", "sibling"]),
                root / "sibling.rs",
            )

    def test_rejects_match_arms_with_different_router_bindings(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text(
                """
                pub fn build_router() -> Router {
                    let primary = Router::new().route("/primary", get(handler));
                    let fallback = Router::new().route("/fallback", get(handler));
                    match use_primary {
                        true => primary,
                        false => fallback,
                    }
                }
                """,
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "runtime-dependent Router match"):
                route_parity.extract_rust_routes(root)

    def test_rejects_early_returns_before_the_router_tail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text(
                """
                pub fn build_router() -> Router {
                    let app = Router::new().route("/mounted", get(handler));
                    if disabled {
                        return Router::new();
                    }
                    trace!("router enabled");
                    app
                }
                """,
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "early return"):
                route_parity.extract_rust_routes(root)

    def test_rejects_top_level_router_reassignment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text(
                """
                pub fn build_router() -> Router {
                    let mut app = Router::new();
                    app = app.route("/extra", get(handler));
                    app
                }
                """,
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "router assignment"):
                route_parity.extract_rust_routes(root)

    def test_rejects_unknown_outer_router_chain_methods(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_text(
                """
                pub fn build_router() -> Router {
                    Router::new()
                        .route("/mounted", get(handler))
                        .fallback(fallback_handler)
                }
                """,
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "unsupported Router chain methods"):
                route_parity.extract_rust_routes(root)

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
