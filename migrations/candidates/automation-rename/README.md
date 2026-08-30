# Automation schema rename (numbering pending)

This directory is an intentionally unnumbered migration candidate. The
migration runner only discovers `*.up.sql` and `*.down.sql` files directly
under `migrations/`, so this candidate is inert until the coordination order
is stable and the final contiguous version is assigned.

The candidate moves the persisted product spelling from the historical
`autopilot*` database objects to the canonical `automation*` objects used by
the current source tree. It renames tables, columns, indexes, and constraints
without adding foreign keys or cascades. It also relabels the two persisted
discriminators that are part of the product contract:

- `issue.origin_type`: `autopilot` becomes `automation`.
- `issue_subscriber.reason`: `autopilot` becomes `automation`.

No arbitrary JSON payloads or historical migration files are rewritten. The
old spelling in this candidate is migration input only; it is not an API,
event, permission, telemetry, or UI compatibility surface.

## Promotion gate

Do not move these files into `migrations/` or assign a number until #629,
Dependency Graph, and Work Product have landed in the final order. At that
point compute the maximum version from the actual final `main` tree and use
exactly `max + 1` for both files. The current working estimate is `430`, not
a reservation.

Before promotion, verify that the final database has every migration through
that maximum recorded and that no canonical target object already exists.
Capture representative row counts for every renamed table, plus counts of
`issue.origin_type = 'autopilot'` and `issue_subscriber.reason = 'autopilot'`.

## Upgrade and downgrade acceptance

Run the upgrade under the migration runner's advisory lock with application
writes drained. Assert all of the following after the upgrade:

1. Every table and column in the source queries exists under its canonical
   name, including `agent_task_queue.automation_run_id` and
   `webhook_delivery.automation_id` / `automation_run_id`.
2. The renamed-table row counts and primary-key values are unchanged.
3. The two discriminator counts are unchanged and their values are now
   `automation`; their checks are validated and no longer accept the old
   spelling.
4. Canonical indexes/constraints exist, old names do not exist in the live
   schema, and `pg_constraint.convalidated` is true for the recreated checks.
5. A fresh Automation create, trigger, run lookup, webhook lookup, quota
   reservation, and Agent-thread continuation can all prepare their current
   SQL paths against the upgraded schema.

Run the down migration only with the canonical binary stopped. Assert the
reverse object names, discriminator values/checks, row counts, and current
old-client SQL fixtures. Then re-apply the upgrade and repeat the assertions
to prove upgrade -> downgrade -> upgrade is reversible.

## Rolling deployment boundary

This product migration deliberately has no old API/event/entitlement adapter
and no supported production users or durable legacy URLs. Therefore a mixed
old/new application rollout against one database is not an accepted mode:
the old binary must be drained before the rename and the canonical binary
started only after the upgrade. If a future deployment requires true rolling
compatibility, stop promotion and design a separately owned, observable,
time-boxed adapter rather than weakening this canonical-only migration.

The old names may remain only in immutable historical migrations, the
historical migration index map, and this candidate until it is promoted and
removed. The owner is the Cordy migration maintainers; the deletion condition
for this directory is the first stable release containing the numbered
migration plus its upgrade/downgrade evidence.
