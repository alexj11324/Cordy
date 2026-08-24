#!/usr/bin/env python3
"""Compare Axum routes with the executable Go router contract."""

from __future__ import annotations

import argparse
import collections
import re
import sys
from pathlib import Path


METHOD_PATTERN = re.compile(
    r"(?<![A-Za-z_])(get|post|put|patch|delete)\s*\(", re.IGNORECASE
)
FUNCTION_PATTERN = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^{};]*>)?\s*\(")
QUALIFIED_CALL_PATTERN = re.compile(
    r"(?<![A-Za-z0-9_.])"
    r"((?:r#)?[A-Za-z_][A-Za-z0-9_]*"
    r"(?:::(?:r#)?[A-Za-z_][A-Za-z0-9_]*)+)"
    r"\s*(?:::<[^{}()]*>)?\s*\("
)
CFG_PATTERN = re.compile(
    r"#\s*\[\s*cfg\s*\((?P<predicate>[^\]]*)\)\s*\]", re.DOTALL
)
WILDCARD_PARAMETER_PATTERN = re.compile(r"\{\*[^{}]+\}")
PARAMETER_PATTERN = re.compile(r"\{[^{}]+\}")
EXPECTED_CONTRACT_SIZE = 424


def method_call_starts(source: str, method: str):
    """Yield method-call offsets that occur in Rust code, not comments/strings."""
    needle = f".{method}("
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
        if source.startswith(needle, cursor):
            yield cursor
            cursor += len(needle)
            continue
        cursor += 1


def method_calls(source: str, method: str):
    """Yield balanced method-call argument strings from Rust source."""
    needle = f".{method}("
    for start in method_call_starts(source, method):
        cursor = start + len(needle)
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
            raise ValueError(f"unterminated {needle} call at byte {start}")

        yield source[argument_start : cursor - 1]


def route_call_starts(source: str):
    yield from method_call_starts(source, "route")


def route_calls(source: str):
    yield from method_calls(source, "route")


def normalize_route(route: str) -> str:
    if route != "/":
        route = route.rstrip("/")
    wildcard = "__CORDY_ROUTE_WILDCARD__"
    route = WILDCARD_PARAMETER_PATTERN.sub(wildcard, route)
    route = re.sub(r"/\*(?=/|$)", f"/{wildcard}", route)
    return PARAMETER_PATTERN.sub("{}", route).replace(wildcard, "{*}")


def extract_routes(source: str) -> set[tuple[str, str]]:
    routes: set[tuple[str, str]] = set()
    for call in route_calls(source):
        match = re.match(r'\s*"([^"\\]+)"\s*,(.*)', call, re.DOTALL)
        if not match:
            continue
        route, methods = match.groups()
        for method in top_level_routing_methods(methods):
            routes.add((method.upper(), normalize_route(route)))
    return routes


def rust_code_mask(source: str) -> str:
    """Mask Rust comments and literals while preserving offsets and newlines."""
    masked = list(source)

    def blank(start: int, end: int) -> None:
        masked[start:end] = ["\n" if char == "\n" else " " for char in masked[start:end]]

    cursor = 0
    while cursor < len(source):
        if source.startswith("//", cursor):
            end = source.find("\n", cursor + 2)
            end = len(source) if end < 0 else end
            blank(cursor, end)
            cursor = end
            continue
        if source.startswith("/*", cursor):
            depth = 1
            end = cursor + 2
            while end < len(source) and depth:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            blank(cursor, end)
            cursor = end
            continue

        raw = (
            re.match(r'(?:br|rb|r)(?P<hashes>#{0,255})"', source[cursor:])
            if source[cursor] in {"b", "r"}
            else None
        )
        if raw and (cursor == 0 or not (source[cursor - 1].isalnum() or source[cursor - 1] == "_")):
            terminator = '"' + raw.group("hashes")
            end = source.find(terminator, cursor + raw.end())
            end = len(source) if end < 0 else end + len(terminator)
            blank(cursor, end)
            cursor = end
            continue

        if source[cursor] == '"':
            end = cursor + 1
            escaped = False
            while end < len(source):
                char = source[end]
                end += 1
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    break
            blank(cursor, end)
            cursor = end
            continue

        if source[cursor] == "'":
            end = cursor + 1
            if end < len(source) and source[end] == "\\":
                end += 2
                if end < len(source) and source[end - 1] == "u" and source[end] == "{":
                    closing = source.find("}", end + 1)
                    end = len(source) if closing < 0 else closing + 1
            else:
                end += 1
            if end < len(source) and source[end] == "'":
                end += 1
                blank(cursor, end)
                cursor = end
                continue

        cursor += 1

    return "".join(masked)


def matching_delimiter(masked: str, opening: int, left: str, right: str) -> int:
    depth = 1
    cursor = opening + 1
    while cursor < len(masked):
        if masked[cursor] == left:
            depth += 1
        elif masked[cursor] == right:
            depth -= 1
            if depth == 0:
                return cursor
        cursor += 1
    raise ValueError(f"unterminated {left} at byte {opening}")


def matching_brace(masked: str, opening: int) -> int:
    return matching_delimiter(masked, opening, "{", "}")


def production_source(source: str) -> str:
    """Blank code that is not guaranteed to exist in a production build."""
    masked = rust_code_mask(source)
    production = list(source)

    def blank(start: int, end: int) -> None:
        production[start:end] = [
            "\n" if char == "\n" else " " for char in production[start:end]
        ]

    for attribute in CFG_PATTERN.finditer(masked):
        if all(
            char.isspace()
            for char in production[attribute.start() : attribute.end()]
        ):
            continue
        predicate = attribute.group("predicate")
        normalized_predicate = re.sub(r"\s+", "", predicate)
        if normalized_predicate == "not(test)":
            continue
        if normalized_predicate != "test":
            compact = " ".join(predicate.split())
            raise ValueError(f"unsupported cfg predicate: {compact}")

        cursor = attribute.end()
        while cursor < len(masked) and masked[cursor].isspace():
            cursor += 1

        round_depth = 0
        square_depth = 0
        end = cursor
        while end < len(masked):
            char = masked[end]
            if char == "(":
                round_depth += 1
            elif char == ")":
                round_depth -= 1
            elif char == "[":
                square_depth += 1
            elif char == "]":
                square_depth -= 1
            elif char == "{" and not (round_depth or square_depth):
                end = matching_brace(masked, end) + 1
                if end < len(masked) and masked[end] == ";":
                    end += 1
                break
            elif char == ";" and not (round_depth or square_depth):
                end += 1
                break
            end += 1
        blank(attribute.start(), end)

    return "".join(production)


def top_level_routing_methods(expression: str):
    """Yield HTTP method constructors on a MethodRouter's outer chain."""
    masked = rust_code_mask(expression)
    depth_at = [0] * (len(masked) + 1)
    round_depth = square_depth = brace_depth = 0
    for index, char in enumerate(masked):
        depth_at[index] = round_depth + square_depth + brace_depth
        if char == "(":
            round_depth += 1
        elif char == ")":
            round_depth -= 1
        elif char == "[":
            square_depth += 1
        elif char == "]":
            square_depth -= 1
        elif char == "{":
            brace_depth += 1
        elif char == "}":
            brace_depth -= 1
    for match in METHOD_PATTERN.finditer(masked):
        if depth_at[match.start()] == 0:
            yield match.group(1)


def top_level_method_calls(source: str, method: str):
    """Yield arguments for calls on the mounted Router expression's outer chain."""
    masked = rust_code_mask(source)
    needle = f".{method}("
    cursor = 0
    round_depth = 0
    square_depth = 0
    brace_depth = 0
    while cursor < len(masked):
        if not (round_depth or square_depth or brace_depth) and masked.startswith(
            needle, cursor
        ):
            opening = cursor + len(needle) - 1
            closing = matching_delimiter(masked, opening, "(", ")")
            yield source[opening + 1 : closing]
            cursor = closing + 1
            continue
        char = masked[cursor]
        if char == "(":
            round_depth += 1
        elif char == ")":
            round_depth -= 1
        elif char == "[":
            square_depth += 1
        elif char == "]":
            square_depth -= 1
        elif char == "{":
            brace_depth += 1
        elif char == "}":
            brace_depth -= 1
        cursor += 1


def extract_mounted_routes(expression: str) -> set[tuple[str, str]]:
    routes: set[tuple[str, str]] = set()
    for call in top_level_method_calls(expression, "route"):
        match = re.match(r'\s*"([^"\\]+)"\s*,(.*)', call, re.DOTALL)
        if not match:
            continue
        route, methods = match.groups()
        for method in top_level_routing_methods(methods):
            routes.add((method.upper(), normalize_route(route)))
    return routes


def top_level_functions(source: str) -> dict[str, tuple[int, int, bool]]:
    """Return top-level function body ranges and whether they return Router."""
    masked = rust_code_mask(source)
    depth_at = [0] * (len(masked) + 1)
    depth = 0
    for index, char in enumerate(masked):
        depth_at[index] = depth
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
    depth_at[len(masked)] = depth

    functions: dict[str, tuple[int, int, bool]] = {}
    for match in FUNCTION_PATTERN.finditer(masked):
        if depth_at[match.start()] != 0:
            continue
        opening = masked.find("{", match.end())
        if opening < 0:
            continue
        semicolon = masked.find(";", match.end(), opening)
        if semicolon >= 0:
            continue
        closing = matching_brace(masked, opening)
        signature = masked[match.end() : opening]
        return_type = signature.split("->", 1)[1] if "->" in signature else ""
        name = match.group(1)
        if name in functions:
            raise ValueError(f"duplicate top-level function {name}")
        functions[name] = (
            opening + 1,
            closing,
            re.search(r"\bRouter\b", return_type) is not None,
        )
    return functions


def source_module_parts(source_root: Path, source_path: Path) -> list[str]:
    """Return the Rust module path represented by a source file."""
    relative = source_path.relative_to(source_root)
    if relative.name in {"lib.rs", "main.rs"}:
        return []
    if relative.name == "mod.rs":
        return list(relative.parent.parts)
    return list(relative.with_suffix("").parts)


def module_source(
    source_root: Path, source_path: Path, parts: list[str]
) -> Path | None:
    """Resolve a module path using the calling Rust module as its base."""
    normalized = [part.removeprefix("r#") for part in parts]
    current = source_module_parts(source_root, source_path)
    if normalized and normalized[0] == "crate":
        current = []
        normalized.pop(0)
    elif normalized and normalized[0] == "self":
        normalized.pop(0)
    else:
        while normalized and normalized[0] == "super":
            if not current:
                raise ValueError(f"{source_path}: module path escapes source root")
            current.pop()
            normalized.pop(0)

    target_parts = [*current, *normalized]
    if not target_parts:
        root = source_root / "lib.rs"
        return root if root.is_file() else None
    direct = source_root.joinpath(*target_parts).with_suffix(".rs")
    if direct.is_file():
        return direct
    nested = source_root.joinpath(*target_parts, "mod.rs")
    return nested if nested.is_file() else None


def router_function_body(body: str) -> tuple[dict[str, str], str]:
    """Return top-level `let` bindings and the function's tail expression."""
    masked = rust_code_mask(body)
    bindings: dict[str, str] = {}
    depth_at = [0] * (len(masked) + 1)
    round_depth = 0
    square_depth = 0
    brace_depth = 0
    top_level_semicolons: list[int] = []

    for index, char in enumerate(masked):
        depth_at[index] = round_depth + square_depth + brace_depth
        if char == "(":
            round_depth += 1
        elif char == ")":
            round_depth -= 1
        elif char == "[":
            square_depth += 1
        elif char == "]":
            square_depth -= 1
        elif char == "{":
            brace_depth += 1
        elif char == "}":
            brace_depth -= 1
        elif char == ";" and not (round_depth or square_depth or brace_depth):
            top_level_semicolons.append(index)
    depth_at[len(masked)] = round_depth + square_depth + brace_depth

    if round_depth or square_depth or brace_depth:
        raise ValueError("unbalanced router function body")

    for match in re.finditer(
        r"(?<![A-Za-z0-9_])let\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)"
        r"(?:\s*:[^=;]+)?\s*=",
        masked,
    ):
        if depth_at[match.start()] != 0:
            continue
        semicolon = next(
            (position for position in top_level_semicolons if position > match.end()),
            None,
        )
        if semicolon is None:
            raise ValueError(f"unterminated top-level let binding {match.group(1)}")
        value_start = match.end()
        bindings[match.group(1)] = body[value_start:semicolon].strip()

    tail_start = top_level_semicolons[-1] + 1 if top_level_semicolons else 0
    tail = body[tail_start:].strip()
    if not tail:
        raise ValueError("router function has no tail expression")
    return bindings, tail


def match_arm_expressions(expression: str) -> list[str]:
    """Return top-level arm expressions from a Router-valued `match`."""
    masked = rust_code_mask(expression)
    stripped = masked.lstrip()
    offset = len(masked) - len(stripped)
    if not stripped.startswith("match "):
        return []

    opening = masked.find("{", offset + len("match "))
    if opening < 0:
        raise ValueError("match expression has no body")
    closing = matching_brace(masked, opening)
    arms_source = expression[opening + 1 : closing]
    arms_masked = masked[opening + 1 : closing]
    boundaries: list[int] = []
    round_depth = 0
    square_depth = 0
    brace_depth = 0
    for index, char in enumerate(arms_masked):
        if char == "(":
            round_depth += 1
        elif char == ")":
            round_depth -= 1
        elif char == "[":
            square_depth += 1
        elif char == "]":
            square_depth -= 1
        elif char == "{":
            brace_depth += 1
        elif char == "}":
            brace_depth -= 1
        elif char == "," and not (round_depth or square_depth or brace_depth):
            boundaries.append(index)

    expressions: list[str] = []
    start = 0
    for end in [*boundaries, len(arms_source)]:
        arm_source = arms_source[start:end]
        arm_masked = arms_masked[start:end]
        round_depth = square_depth = brace_depth = 0
        arrow = None
        cursor = 0
        while cursor + 1 < len(arm_masked):
            char = arm_masked[cursor]
            if char == "(":
                round_depth += 1
            elif char == ")":
                round_depth -= 1
            elif char == "[":
                square_depth += 1
            elif char == "]":
                square_depth -= 1
            elif char == "{":
                brace_depth += 1
            elif char == "}":
                brace_depth -= 1
            elif (
                arm_masked.startswith("=>", cursor)
                and not (round_depth or square_depth or brace_depth)
            ):
                arrow = cursor
                break
            cursor += 1
        if arm_source.strip():
            if arrow is None:
                raise ValueError("cannot parse match arm in router expression")
            expressions.append(arm_source[arrow + 2 :].strip())
        start = end + 1
    if not expressions:
        raise ValueError("router match expression has no arms")
    return expressions


def passthrough_router_base(expression: str) -> str | None:
    """Return the shared Router binding for a route-preserving outer chain."""
    masked = rust_code_mask(expression)
    base = re.match(r"\s*([A-Za-z_][A-Za-z0-9_]*)", masked)
    if base is None:
        return None
    cursor = base.end()
    while True:
        while cursor < len(masked) and masked[cursor].isspace():
            cursor += 1
        if cursor == len(masked):
            return base.group(1)
        if masked[cursor] != ".":
            return None
        method = re.match(r"\.([A-Za-z_][A-Za-z0-9_]*)\s*\(", masked[cursor:])
        if method is None or method.group(1) not in {
            "layer",
            "route_layer",
            "with_state",
        }:
            return None
        opening = cursor + method.end() - 1
        cursor = matching_delimiter(masked, opening, "(", ")") + 1


def extract_rust_routes(source_root: Path) -> set[tuple[str, str]]:
    """Extract routes reachable from the production `build_router` entrypoint."""
    root_source = source_root / "lib.rs"
    if not root_source.is_file():
        raise ValueError(f"{root_source}: missing router entrypoint source")

    source_cache: dict[Path, str] = {}
    function_cache: dict[Path, dict[str, tuple[int, int, bool]]] = {}

    def load(path: Path) -> tuple[str, dict[str, tuple[int, int, bool]]]:
        if path not in source_cache:
            source_cache[path] = production_source(path.read_text(encoding="utf-8"))
            function_cache[path] = top_level_functions(source_cache[path])
        return source_cache[path], function_cache[path]

    routes: set[tuple[str, str]] = set()
    pending = [(root_source, "build_router")]
    visited: set[tuple[Path, str]] = set()
    while pending:
        source_path, function_name = pending.pop()
        key = (source_path, function_name)
        if key in visited:
            continue
        visited.add(key)

        source, functions = load(source_path)
        function = functions.get(function_name)
        if function is None:
            raise ValueError(f"{source_path}: missing router function {function_name}")
        start, end, _ = function
        bindings, tail = router_function_body(source[start:end])
        seen_bindings: set[str] = set()

        def walk_expression(expression: str, require_router: bool = False) -> bool:
            masked_expression = rust_code_mask(expression)
            if any(
                next(top_level_method_calls(expression, unsupported), None) is not None
                for unsupported in ("nest", "nest_service", "route_service")
            ):
                raise ValueError(
                    f"{source_path}: unsupported mounted router composition in {function_name}"
                )

            routes.update(extract_mounted_routes(expression))
            stripped = masked_expression.lstrip()
            found_router = re.match(r"Router::new\s*\(", stripped) is not None

            match_arms = match_arm_expressions(expression)
            if match_arms:
                bases = [passthrough_router_base(arm) for arm in match_arms]
                if (
                    any(base is None or base not in bindings for base in bases)
                    or len(set(bases)) != 1
                ):
                    raise ValueError(
                        f"{source_path}: runtime-dependent Router match branches "
                        "are unsupported"
                    )
                name = bases[0]
                assert name is not None
                if name in seen_bindings:
                    raise ValueError(f"{source_path}: cyclic router binding {name}")
                seen_bindings.add(name)
                if not walk_expression(bindings[name], True):
                    raise ValueError(f"{source_path}: unresolved Router match base {name}")
                seen_bindings.remove(name)
                found_router = True
            elif stripped.startswith("if "):
                raise ValueError(
                    f"{source_path}: conditional mounted router expressions are unsupported"
                )

            base = re.match(r"([A-Za-z_][A-Za-z0-9_]*)\s*(?=\.|\Z)", stripped)
            if base and base.group(1) in bindings:
                name = base.group(1)
                if name in seen_bindings:
                    raise ValueError(f"{source_path}: cyclic router binding {name}")
                seen_bindings.add(name)
                found_router = walk_expression(bindings[name], True) or found_router
                seen_bindings.remove(name)

            for name, (_, _, returns_router) in functions.items():
                if returns_router and re.match(
                    rf"{re.escape(name)}\s*\(", stripped
                ):
                    pending.append((source_path, name))
                    found_router = True

            qualified = QUALIFIED_CALL_PATTERN.match(stripped)
            if qualified:
                match = qualified
                parts = [part.removeprefix("r#") for part in match.group(1).split("::")]
                target = module_source(source_root, source_path, parts[:-1])
                if target is not None:
                    _, target_functions = load(target)
                    target_function = target_functions.get(parts[-1])
                    if target_function is not None and target_function[2]:
                        pending.append((target, parts[-1]))
                        found_router = True

            for argument in top_level_method_calls(expression, "merge"):
                if not walk_expression(argument, True):
                    snippet = " ".join(argument.split())[:120]
                    raise ValueError(
                        f"{source_path}: unsupported .merge router expression {snippet!r}"
                    )

            if require_router and not found_router:
                return False
            return found_router

        if not walk_expression(tail, True):
            raise ValueError(
                f"{source_path}: cannot resolve mounted router returned by {function_name}"
            )

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
        default=server_rs / "route-contract" / "go-routes.tsv",
    )
    parser.add_argument(
        "--rust-source",
        type=Path,
        default=server_rs / "crates" / "cordy-handler" / "src",
    )
    parser.add_argument(
        "--require-complete",
        action="store_true",
        help="exit non-zero unless Rust exactly matches the Go contract",
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
        f"Go contract: {len(expected)} | Rust: {len(actual)} | "
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
