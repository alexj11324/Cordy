# Go route contract

`go-routes.tsv` is the normalized output of walking the executable Go Chi
router. It is the compatibility boundary for the Axum migration: HTTP methods,
path parameters, and trailing slashes are normalized, while every distinct
method/path pair remains required.

Run the audit while migrating:

```bash
python3 server-rs/scripts/route_parity.py
```

Use the strict gate before declaring the handler migration complete:

```bash
python3 server-rs/scripts/route_parity.py --storage-backend local --require-complete
python3 server-rs/scripts/route_parity.py --storage-backend object --require-complete
```

Strict audits require the target storage backend because the Go server mounts
`GET /uploads/*` only for local storage. Contract rows may use a third TSV
column such as `storage=local` to describe that deployment condition.

Regenerate the contract only from a reviewed change to the executable Go
router. A task-document estimate is not sufficient evidence for changing this
fixture.
