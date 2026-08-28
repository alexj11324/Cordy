# Route contract

`routes.tsv` is the canonical normalized production HTTP route inventory.
HTTP methods, path parameters, and trailing slashes are normalized, while
every distinct method/path pair remains required.

Run the audit after changing production routes:

```bash
python3 server-rs/scripts/route_parity.py
```

Use the strict gate before declaring route work complete:

```bash
python3 server-rs/scripts/route_parity.py --require-complete
```

Regenerate the contract only from a reviewed production routing change. A
task-document estimate is not sufficient evidence for changing this fixture.
