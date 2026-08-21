#!/usr/bin/env python3
"""Generate Rust DB layer from sqlc-generated Go code (v3).

Fixes over v2:
  - Bare `error` returns (no parens) now match -> recovers ~170 exec fns
  - Full param type map incl. []pgtype.UUID, []string, pgtype.Bool/Int4/Int8/
    Float8/Interval, *netip.Addr, float64
  - Robust Scan-target parsing (receiver var name varies; inline Scan calls)
  - pret normalization handles "(X, error)" / "error" uniformly
"""
import os, re, json
from collections import defaultdict

ROOT = "/Users/alexjiang/Desktop/vibe/Cordy"
GEN = f"{ROOT}/server/pkg/db/generated"
OUT = f"{ROOT}/server-rs/crates/cordy-db/src"
SCHEMA_DUMP = "/tmp/schema_dump.txt"

# ---------- schema ----------
schema = {}
for line in open(SCHEMA_DUMP):
    parts = line.strip().split("|")
    if len(parts) == 5:
        t, c, dt, nul, udt = parts
        schema[(t, c)] = (udt, nul == "YES")

tables = defaultdict(list)
for (t, c), (udt, nul) in sorted(schema.items()):
    tables[t].append((c, udt, nul))

RUST_KEYWORDS = {
    "as","break","const","continue","crate","else","enum","extern","false",
    "fn","for","if","impl","in","let","loop","match","mod","move","mut",
    "pub","ref","return","self","self_","static","struct","super","trait",
    "true","type","unsafe","use","where","while","async","await","dyn","box",
}

def snake(name):
    s = re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", name)
    s = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", s)
    s = s.lower()
    return s + "_" if s in RUST_KEYWORDS else s

RS_BASE = {
    "uuid": "Uuid", "text": "String", "varchar": "String",
    "timestamptz": "DateTime<Utc>", "timestamp": "DateTime<Utc>",
    "date": "chrono::NaiveDate",
    "bool": "bool", "int2": "i16", "int4": "i32", "int8": "i64",
    "float8": "f64", "jsonb": "serde_json::Value", "json": "serde_json::Value",
    "bytea": "Vec<u8>", "_uuid": "Vec<Uuid>", "_text": "Vec<String>",
    "inet": "String",
}

def rs_type(udt, nullable):
    base = RS_BASE.get(udt)
    if base is None:
        return None
    return f"Option<{base}>" if nullable else base

def gen_models():
    header = ("//! Table models generated from the live schema (information_schema).\n"
              "//! Mirrors server/pkg/db/generated/models.go.\n\n"
              "use chrono::{DateTime, Utc};\nuse serde::Serialize;\nuse uuid::Uuid;\n\n")
    out, skipped = [header], []
    for t, cols in sorted(tables.items()):
        sn = snake(t)
        struct_name = "".join(w.capitalize() for w in sn.split("_"))
        if struct_name.endswith("S"):
            struct_name = struct_name[:-1]
        block = [f"/// Row of `{t}`.\n#[derive(Debug, Clone, Serialize, sqlx::FromRow)]\npub struct {struct_name} {{\n"]
        ok = True
        for c, udt, nul in cols:
            ty = rs_type(udt, bool(nul))
            if ty is None:
                skipped.append((t, c, udt)); ok = False; break
            block.append(f"    pub {snake(c)}: {ty},\n")
        if ok:
            out += block + ["}\n\n"]
    return "".join(out), skipped

# ---------- go parsing ----------
GO_FIELD = re.compile(r"^\s*(\w+)\s+(\S+)\s+`json")
CONST_RE = re.compile(r"const (\w+) = `-- name: (\w+) :(\w+)\n(.*?)`\n", re.S)
STRUCT_RE = re.compile(r"type (\w+(?:Params|Row)) struct \{\n(.*?)\n\}", re.S)
FUNC_RE = re.compile(
    r"func \(q \*Queries\) (\w+)\(ctx context\.Context(?:, ([^)]*))?\)\s*(.*?)\{\n(.*?)\n\}", re.S)

def parse_struct_fields(body):
    fields = []
    for line in body.split("\n"):
        fm = GO_FIELD.match(line)
        if fm:
            fields.append((fm.group(1), fm.group(2)))
    return fields

def load_models():
    src = open(f"{GEN}/models.go").read()
    return {m.group(1): parse_struct_fields(m.group(2))
            for m in re.finditer(r"type (\w+) struct \{\n(.*?)\n\}", src, re.S)}

# param types (function signatures) — value position
PARAM_MAP = {
    "pgtype.UUID": "Uuid",
    "[]uuid.UUID": "&[Uuid]",
    "[]pgtype.UUID": "Vec<Uuid>",
    "[]string": "&[String]",
    "string": "&str",
    "int64": "i64", "int32": "i32",
    "bool": "bool",
    "float64": "f64",
    "[]byte": "&serde_json::Value",
    "interface{}": "&serde_json::Value",
    "pgtype.Text": "Option<&str>",
    "pgtype.Timestamptz": "Option<DateTime<Utc>>",
    "pgtype.Date": "Option<chrono::NaiveDate>",
    "pgtype.Bool": "Option<bool>",
    "pgtype.Int4": "Option<i32>",
    "pgtype.Int8": "Option<i64>",
    "pgtype.Float8": "Option<f64>",
    "pgtype.Interval": "sqlx::postgres::types::PgInterval",
    "*netip.Addr": "Option<ipnetwork::IpNetwork>",
}

# scalar result types — pgtype.* means nullable per sqlc inference
SCALAR_MAP = {
    "pgtype.UUID": "Option<Uuid>",
    "pgtype.Text": "Option<String>",
    "pgtype.Timestamptz": "Option<DateTime<Utc>>",
    "pgtype.Date": "Option<chrono::NaiveDate>",
    "pgtype.Bool": "Option<bool>",
    "pgtype.Int4": "Option<i32>",
    "pgtype.Int8": "Option<i64>",
    "pgtype.Float8": "Option<f64>",
    "pgtype.Interval": "Option<sqlx::postgres::types::PgInterval>",
    "[]byte": "Option<serde_json::Value>",
    "interface{}": "serde_json::Value",
    "string": "String",
    "int64": "i64", "int32": "i32",
    "bool": "bool",
    "float64": "f64",
    "uuid.UUID": "Uuid",
    "[]pgtype.UUID": "Option<Vec<Uuid>>",
    "[]string": "Option<Vec<String>>",
}

def go_param_to_rs(gotype):
    return PARAM_MAP.get(gotype)

def go_scalar_to_rs(gotype):
    return SCALAR_MAP.get(gotype)

def normalize_pret(pret):
    """'(User, error)' -> 'User'; 'error' -> None; '[]X, error' -> '[]X'."""
    p = pret.strip()
    if p.startswith("(") and p.endswith(")"):
        p = p[1:-1].strip()
    p = re.sub(r",\s*error\s*$", "", p).strip()
    if p == "error":
        return None
    return p

def parse_scan_args(body):
    """Return list of scan targets: ('field', recv, FieldName) | ('local', name)."""
    m = re.search(r"\.Scan\(([^)]*)\)", body, re.S)
    if not m:
        return None
    args = [a.strip() for a in m.group(1).split(",") if a.strip()]
    out = []
    for a in args:
        if not a.startswith("&"):
            out.append(("raw", a)); continue
        inner = a[1:]
        dm = re.match(r"(\w+)\.(\w+)$", inner)
        if dm:
            out.append(("field", dm.group(2)))
        else:
            out.append(("local", inner))
    return out

def main():
    models_rs, skipped_tables = gen_models()
    open(f"{OUT}/models.rs", "w").write(models_rs)
    table_models = load_models()

    qdir = f"{OUT}/queries"
    os.makedirs(qdir, exist_ok=True)
    mods, stats, unsupported = [], defaultdict(int), []

    for fn in sorted(os.listdir(GEN)):
        if not fn.endswith(".sql.go"):
            continue
        src = open(f"{GEN}/{fn}").read()
        mod_name = fn[:-7]

        consts = {m.group(1): (m.group(2), m.group(3), m.group(4).strip())
                  for m in CONST_RE.finditer(src)}
        aux_structs = {}
        for m in STRUCT_RE.finditer(src):
            aux_structs[m.group(1)] = parse_struct_fields(m.group(2))

        rust_fns = []
        for fm in FUNC_RE.finditer(src):
            gname, psig, pret_raw, body = fm.groups()
            cname = gname[0].lower() + gname[1:]
            if cname not in consts:
                unsupported.append((mod_name, gname, "const-missing", cname))
                continue
            _, ann, sql = consts[cname]
            rust_name = snake(gname)

            params = []
            if psig and psig.strip():
                for piece in psig.split(","):
                    piece = piece.strip()
                    pm = re.match(r"arg (\w+)$", piece)
                    if pm:
                        params += aux_structs.get(pm.group(1),
                                                  table_models.get(pm.group(1), []))
                    else:
                        nm, ty = piece.rsplit(" ", 1)
                        params.append((nm, ty))

            rust_params, bind_list, bad_param = [], [], False
            for pname, ptype in params:
                rt = go_param_to_rs(ptype)
                if rt is None:
                    unsupported.append((mod_name, gname, "param", ptype))
                    bad_param = True
                    break
                pn = snake(pname)
                rust_params.append(f"{pn}: {rt}")
                bind_list.append(pn)

            if bad_param:
                unsupported.append((mod_name, gname, "skipped-bad-param", ""))
                continue

            binds = "".join(f"\n        .bind({b})" for b in bind_list)
            sig_prefix = (f"pub async fn {rust_name}(executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>"
                          f"{', ' if rust_params else ''}{', '.join(rust_params)})")

            # ---- exec family ----
            if ann in ("exec", "execrows", "copyfrom"):
                stats["exec"] += 1
                rust_fns.append(
                    f"{sig_prefix} -> anyhow::Result<u64> {{\n"
                    f"    let r = sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                    f"        .execute(executor)\n        .await?;\n    Ok(r.rows_affected())\n}}")
                continue

            ret_ty_go = normalize_pret(pret_raw)
            if ret_ty_go is None:
                stats["exec"] += 1
                rust_fns.append(
                    f"{sig_prefix} -> anyhow::Result<()> {{\n"
                    f"    sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                    f"        .execute(executor)\n        .await?;\n    Ok(())\n}}")
                continue

            is_many = ret_ty_go.startswith("[]")
            elem_ty_go = ret_ty_go[2:] if is_many else ret_ty_go

            fetch_call = "fetch_all(executor)" if is_many else "fetch_optional(executor)"

            def emit_model(model_name):
                fields = table_models[model_name]
                ctor = ", ".join(f"{snake(f)}: row.try_get({i})?"
                                 for i, (f, _) in enumerate(fields))
                if is_many:
                    stats["many_model"] += 1
                    return (f"{sig_prefix} -> anyhow::Result<Vec<{model_name}>> {{\n"
                            f"    let rows = sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                            f"        .fetch_all(executor)\n        .await?;\n"
                            f"    let mut out = Vec::with_capacity(rows.len());\n"
                            f"    for row in &rows {{\n"
                            f"        out.push({model_name} {{ {ctor} }});\n"
                            f"    }}\n"
                            f"    Ok(out)\n}}")
                stats["one_model"] += 1
                return (f"{sig_prefix} -> anyhow::Result<Option<{model_name}>> {{\n"
                        f"    let row = sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                        f"        .fetch_optional(executor)\n        .await?;\n"
                        f"    let Some(row) = row else {{ return Ok(None) }};\n"
                        f"    Ok(Some({model_name} {{ {ctor} }}))\n}}")

            if elem_ty_go in table_models:
                rust_fns.append(emit_model(elem_ty_go))
                continue

            # Row struct return?
            if elem_ty_go in aux_structs and elem_ty_go.endswith("Row"):
                fields = aux_structs[elem_ty_go]
                field_rs, ctor_items, bad = [], [], False
                idx = 0
                for fname, ftype in fields:
                    if ftype in table_models:
                        # embedded table model: consumes len(fields) columns
                        sub = table_models[ftype]
                        sub_ctor = ", ".join(
                            f"{snake(sf)}: row.try_get({idx + j})?"
                            for j, (sf, _) in enumerate(sub))
                        frs = snake(fname)
                        field_rs.append(f"    pub {frs}: {ftype},")
                        ctor_items.append(f"{frs}: {ftype} {{ {sub_ctor} }}")
                        idx += len(sub)
                        continue
                    rt = SCALAR_MAP.get(ftype)
                    if rt is None:
                        unsupported.append((mod_name, elem_ty_go, "rowfield", ftype))
                        bad = True; break
                    field_rs.append(f"    pub {snake(fname)}: {rt},")
                    ctor_items.append(f"{snake(fname)}: row.try_get({idx})?")
                    idx += 1
                if bad:
                    unsupported.append((mod_name, gname, "skipped-bad-row", ""))
                    continue
                row_def = ("#[derive(Debug, Clone, serde::Serialize)]\n"
                           f"pub struct {elem_ty_go} {{\n" + "\n".join(field_rs) + "\n}\n\n")
                if is_many:
                    stats["many_row"] += 1
                    rust_fns.append(
                        f"{row_def}"
                        f"{sig_prefix} -> anyhow::Result<Vec<{elem_ty_go}>> {{\n"
                        f"    let rows = sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                        f"        .fetch_all(executor)\n        .await?;\n"
                        f"    let mut out = Vec::with_capacity(rows.len());\n"
                        f"    for row in &rows {{\n"
                        f"        out.push({elem_ty_go} {{ "
                        + ", ".join(ctor_items) + " });\n"
                        f"    }}\n"
                        f"    Ok(out)\n}}")
                else:
                    stats["one_row"] += 1
                    rust_fns.append(
                        f"{row_def}"
                        f"{sig_prefix} -> anyhow::Result<Option<{elem_ty_go}>> {{\n"
                        f"    let row = sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                        f"        .fetch_optional(executor)\n        .await?;\n"
                        f"    let Some(row) = row else {{ return Ok(None) }};\n"
                        f"    Ok(Some({elem_ty_go} {{ " + ", ".join(ctor_items) + " }))\n}")
                continue

            # scalar / tuple
            targets = parse_scan_args(body)
            if targets is None:
                unsupported.append((mod_name, gname, "no-scan", pret_raw))
                continue
            resolved = []
            for kind, val in targets:
                rt = None
                if kind == "local":
                    vm = re.search(rf"var {re.escape(val)} (\S+)", body)
                    if vm:
                        rt = go_scalar_to_rs(vm.group(1))
                elif kind == "field":
                    for fname, ftype in aux_structs.get(elem_ty_go, []):
                        if fname == val:
                            rt = go_scalar_to_rs(ftype)
                            break
                if rt is None:
                    unsupported.append((mod_name, gname, "scan-type", str(val)))
                    rt = "serde_json::Value"
                resolved.append(rt)

            gets_all = ", ".join(f"row.try_get({i})?" for i in range(len(resolved)))
            val_expr = f"({gets_all})" if len(resolved) > 1 else gets_all
            ret_rust = ("(" + ", ".join(resolved) + ")") if len(resolved) > 1 else resolved[0]

            if is_many:
                stats["scalar_many"] += 1
                gets = ", ".join(f"row.try_get({i})?" for i in range(len(resolved)))
                rust_fns.append(
                    f"{sig_prefix} -> anyhow::Result<Vec<{ret_rust}>> {{\n"
                    f"    let rows = sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                    f"        .fetch_all(executor)\n        .await?;\n"
                    f"    let mut out = Vec::with_capacity(rows.len());\n"
                    f"    for row in &rows {{\n"
                    f"        out.push({val_expr});\n"
                    f"    }}\n"
                    f"    Ok(out)\n}}")
            else:
                stats["scalar_one"] += 1
                gets = ", ".join(f"row.try_get({i})?" for i in range(len(resolved)))
                rust_fns.append(
                    f"{sig_prefix} -> anyhow::Result<Option<{ret_rust}>> {{\n"
                    f"    let row = sqlx::query(\n        r#\"{sql}\"#\n    ){binds}\n"
                    f"        .fetch_optional(executor)\n        .await?;\n"
                    f"    let Some(row) = row else {{ return Ok(None) }};\n"
                    f"    Ok(Some({val_expr}))\n}}")

        if rust_fns:
            with open(f"{qdir}/{mod_name}.rs", "w") as f:
                f.write(f"//! Port of server/pkg/db/queries/{mod_name}.sql (generated {mod_name}.sql.go).\n"
                        f"//! Positional extraction mirrors Go's Scan order exactly.\n\n"
                        f"#![allow(clippy::too_many_arguments)]\n"
                        f"#![allow(unused_imports)]\n\n"
                        f"use crate::models::*;\nuse chrono::{{DateTime, Utc}};\n"
                        f"use sqlx::Row;\nuse uuid::Uuid;\n\n"
                        + "\n\n".join(rust_fns) + "\n")
            mods.append(mod_name)

    with open(f"{qdir}/mod.rs", "w") as f:
        f.write("//! Query modules ported from sqlc sources (positional extraction).\n\n")
        for m in sorted(mods):
            f.write(f"pub mod {m};\n")

    print(json.dumps({"modules": len(mods), "stats": dict(stats),
                      "skipped_tables": skipped_tables,
                      "unsupported_count": len(unsupported)}, indent=2))
    json.dump(unsupported, open("/tmp/port_unsupported.json", "w"), indent=2)

if __name__ == "__main__":
    main()
