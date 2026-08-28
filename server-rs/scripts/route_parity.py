#!/usr/bin/env python3
"""Compare Axum routes with the canonical production route contract."""

from __future__ import annotations

import argparse
import collections
import re
import sys
from pathlib import Path


METHOD_PATTERN = re.compile(
    r"(?<![A-Za-z_])(get|post|put|patch|delete)\s*\(", re.IGNORECASE
)
WILDCARD_PARAMETER_PATTERN = re.compile(r"\{\*[^{}]+\}")
PARAMETER_PATTERN = re.compile(r"\{[^{}]+\}")
EXPECTED_CONTRACT_SIZE = 424


def route_call_starts(source: str):
    """Yield `.route(` offsets that occur in Rust code, not comments/strings."""
    cursor = 0
    block_depth = 0
    while cursor < len(source):
        if block_depth:
            if source.startswith("/*", cursor):
                block_depth += 1
                cursor += 2
            elif source.startswith("*/", cursor):
                block_depth -= 1
                cursor += 2
            else:
                cursor += 1
            continue
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor + 2)
            cursor = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith("/*", cursor):
            block_depth = 1
            cursor += 2
            continue
        if source[cursor] == '"':
            cursor += 1
            escaped = False
            while cursor < len(source):
                char = source[cursor]
                cursor += 1
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    break
            continue
        if source.startswith(".route(", cursor):
            yield cursor
            cursor += len(".route(")
            continue
        cursor += 1


def route_calls(source: str):
    """Yield balanced `.route(...)` argument strings from Rust source."""
    for start in route_call_starts(source):
        cursor = start + len(".route(")
        argument_start = cursor
        depth = 1
        in_string = False
        escaped = False
        line_comment = False
        block_depth = 0

        while cursor < len(source) and depth:
            char = source[cursor]
            if line_comment:
                if char == "\n":
                    line_comment = False
            elif block_depth:
                if source.startswith("/*", cursor):
                    block_depth += 1
                    cursor += 1
                elif source.startswith("*/", cursor):
                    block_depth -= 1
                    cursor += 1
            elif in_string:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    in_string = False
            elif source.startswith("//", cursor):
                line_comment = True
                cursor += 1
            elif source.startswith("/*", cursor):
                block_depth = 1
                cursor += 1
            elif char == '"':
                in_string = True
            elif char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
            cursor += 1

        if depth:
            raise ValueError(f"unterminated .route( call at byte {start}")

        yield source[argument_start : cursor - 1]


def normalize_route(route: str) -> str:
    if route != "/":
        route = route.rstrip("/")
    wildcard = "__CORDY_ROUTE_WILDCARD__"
    route = WILDCARD_PARAMETER_PATTERN.sub(wildcard, route)
    route = re.sub(r"/\*(?=/|$)", f"/{wildcard}", route)
    return PARAMETER_PATTERN.sub("{}", route).replace(wildcard, "{*}")


def extract_rust_routes(source_root: Path) -> set[tuple[str, str]]:
    routes: set[tuple[str, str]] = set()
    for source_path in sorted(source_root.rglob("*.rs")):
        source = source_path.read_text(encoding="utf-8")
        for call in route_calls(source):
            match = re.match(r'\s*"([^"\\]+)"\s*,(.*)', call, re.DOTALL)
            if not match:
                continue
            route, methods = match.groups()
            for method in METHOD_PATTERN.findall(methods):
                routes.add((method.upper(), normalize_route(route)))
    return routes


def load_contract(contract_path: Path) -> set[tuple[str, str]]:
    routes: set[tuple[str, str]] = set()
    lines = contract_path.read_text(encoding="utf-8").splitlines()
    for line_number, line in enumerate(lines, start=1):
        if not line or line.startswith("#"):
            continue
        try:
            method, route = line.split("\t", 1)
        except ValueError as error:
            raise ValueError(
                f"{contract_path}:{line_number}: expected METHOD<TAB>/path"
            ) from error
        normalized = (method.upper(), normalize_route(route))
        if normalized in routes:
            raise ValueError(
                f"{contract_path}:{line_number}: duplicate route "
                f"{normalized[0]} {normalized[1]}"
            )
        routes.add(normalized)
    if len(routes) != EXPECTED_CONTRACT_SIZE:
        raise ValueError(
            f"{contract_path}: expected {EXPECTED_CONTRACT_SIZE} routes, "
            f"found {len(routes)}"
        )
    return routes


def domain(route: str) -> str:
    parts = [part for part in route.split("/") if part]
    if not parts:
        return "/"
    if parts[0] == "api" and len(parts) > 1:
        return f"/api/{parts[1]}"
    return f"/{parts[0]}"


def format_route(item: tuple[str, str]) -> str:
    return f"{item[0]}\t{item[1]}"


def main(argv: list[str] | None = None) -> int:
    script_dir = Path(__file__).resolve().parent
    server_rs = script_dir.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--contract",
        type=Path,
        default=server_rs / "route-contract" / "routes.tsv",
    )
    parser.add_argument(
        "--rust-source",
        type=Path,
        default=server_rs / "crates" / "cordy-handler" / "src",
    )
    parser.add_argument(
        "--require-complete",
        action="store_true",
        help="exit non-zero unless Rust exactly matches the route contract",
    )
    parser.add_argument(
        "--list-missing", action="store_true", help="print every missing route"
    )
    args = parser.parse_args(argv)

    try:
        expected = load_contract(args.contract)
        actual = extract_rust_routes(args.rust_source)
    except (OSError, ValueError) as error:
        print(f"route parity error: {error}", file=sys.stderr)
        return 2

    missing = expected - actual
    extra = actual - expected
    print(
        f"Contract: {len(expected)} | Rust: {len(actual)} | "
        f"covered: {len(expected & actual)} | missing: {len(missing)} | extra: {len(extra)}"
    )

    if missing:
        counts = collections.Counter(domain(route) for _, route in missing)
        print("Missing by domain:")
        for name, count in sorted(counts.items(), key=lambda item: (-item[1], item[0])):
            print(f"  {count:3}  {name}")

    if args.list_missing:
        for item in sorted(missing, key=lambda value: (value[1], value[0])):
            print(f"MISSING\t{format_route(item)}")

    for item in sorted(extra, key=lambda value: (value[1], value[0])):
        print(f"EXTRA\t{format_route(item)}")

    if args.require_complete and (missing or extra):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
