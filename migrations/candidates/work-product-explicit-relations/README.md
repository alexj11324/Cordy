# Work Product explicit relations (numbering pending)

This directory is an intentionally unnumbered migration candidate. The
migration runner only reads files directly under `migrations/`, so these files
are inert until the coordinator assigns final versions and moves each unit to
that directory.

The schema is provider-neutral at the identity/relation boundary:

- `work_product` is the idempotent external identity (`workspace_id` +
  `provider` + `external_identity`). Provider mirror rows remain the
  source for GitHub/VCS snapshots and are referenced by type/id.
- `work_product_relation` is the only association table. It records the Issue
  target, server-derived task/run execution context, one of
  `manual_explicit`, `task_explicit`, or `execution_branch_discovery`, actor/time,
  close intent, and detach audit fields. Branch discovery writes this same
  durable relation only after a unique exact-head match; branch text is never
  stored as a lasting relationship key.
- `agent_task_execution_provenance` is the task-owned execution record. It
  stores the authenticated run's repository identity, exact execution
  workspace/head branch, active/finished timestamps, and the observable
  `unassociated`/`ambiguous`/`associated`/`ineligible` discovery result. It is
  not a second association table and never parses PR text.
- No foreign keys or cascades are introduced. Application transactions own
  workspace/issue/task/run checks and dependent cleanup.
- No PB-style identifier is present in the key or schema. External identities
  are provider identities only.
- Manual Attach reuses an existing active relation for the same Work Product
  and Issue when one is already present. Confirming a discovered product can
  therefore promote that relation to `manual_explicit` without discarding the
  producing task/run provenance; it does not create a parallel association.

The 16 core migration units are deliberately split so every index is a single
`CREATE [UNIQUE] INDEX CONCURRENTLY` unit before its primary-key/uniqueness
constraint is attached:

1. `work_product_table`
2. `work_product_id_index`
3. `work_product_primary_key`
4. `work_product_external_identity_index`
5. `work_product_provider_record_index`
6. `work_product_relation_table`
7. `work_product_relation_id_index`
8. `work_product_relation_primary_key`
9. `work_product_relation_key_index`
10. `work_product_relation_issue_index`
11. `work_product_relation_product_index`
12. `work_product_relation_task_index`
13. `agent_task_execution_provenance_table`
14. `agent_task_execution_provenance_task_index`
15. `agent_task_execution_provenance_primary_key`
16. `agent_task_execution_provenance_branch_index`

The provider-record index is required by the current webhook, snapshot, and
VCS status joins that resolve a provider mirror row to its Work Product. There
is deliberately no run or discovery-status index: the current service resolves
those by task id and the exact repository/branch execution predicate. Adding
an index for a field that has no current query consumer would make the
migration larger without improving this feature.

The two old provider-specific relation tables are no longer read or written by
the service. Their final drop must be coordinated as two additional cleanup
units (`issue_pull_request_table_cleanup` and
`issue_vcs_pull_request_table_cleanup`) after all open branches that still
mention them are merged. Those drops intentionally do not backfill data; this
product has no historical relations to preserve. The legacy provider-specific
columns disappear with those retired tables.

The complete candidate set is therefore 18 units: 16 core schema/index units
and the 2 retired provider-relation table cleanup units. This is smaller than
the earlier 20-unit sketch because it does not create unused run or
discovery-status indexes, while retaining every index with a live query
consumer and keeping each concurrent index in its own unit.

Final numbering must be assigned only after the migration sequence supplied by
the coordination task is stable.
