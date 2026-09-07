# Self-hosted identity cutover

Orvilo is a hard deployment-identity cutover. Existing installations must migrate
separately; ordinary image or chart upgrades are supported only after that move.
Keeping the old Compose project name or Helm selectors is not the selected policy.

## Preparation

Record the actual Compose project or Helm release and namespace, image versions,
database credentials, uploads volume/PVC, encryption keys, and external endpoints.
Do not infer these from the current checkout directory or the new defaults.
Inventory every persistent mount, including an external database or object store.
Back up the database and uploads, and prove the backups can be restored into an
isolated target. Keep encryption keys available securely so restored integrations
remain readable.

## Migration

1. Schedule a maintenance window and stop traffic, workers, and daemon writes.
   Take final database and uploads backups after all writers are paused.
2. Stop the old Compose project or scale down the old Helm workloads without
   deleting their volumes/PVCs. Keep their original manifests and configuration.
   Do not use volume-deleting teardown commands.
3. Create a fresh Orvilo Compose project or Helm release. Use new volumes/PVCs
   and separate ports/endpoints during acceptance. Restore the final backups
   before starting the new backend; let its migration runner upgrade the copied
   database. Never attach both old and new database containers to the same data.
4. Configure the renamed environment variables, images, callback protocol,
   hostnames, and credentials. Verify readiness, authenticated workspace access,
   existing issues, uploaded files, integration decryption, and daemon callbacks.
5. Switch traffic only after acceptance. Resume writers on the new deployment.
   Retire old infrastructure only after the operator's rollback window expires.

Changing a Compose project name creates different default volume names; it is
not a data migration. A Helm chart rename does not make Deployment selectors
mutable. Use a new release instead of forcing the old Deployment selector update.

## Client and integration cutover checklist

Before enabling traffic, complete these steps on the isolated restored target:

- Revoke pre-cutover personal access tokens using the old installation before
  the final backup. Issue fresh Orvilo tokens after restoring and update every
  CLI, daemon, realtime consumer, and integration. Old token secrets are not
  accepted and cannot be transformed from their stored hashes.
- Stop old daemons and install the Orvilo CLI/Desktop distributions explicitly.
  The old CLI updater cannot consume renamed archives. Configure fresh Orvilo
  profiles; do not rely on old userData or credential directories being reused.
- Rebuild and publish every installed plugin bundle with the Orvilo SDK, then
  explicitly upgrade each installation before enabling its surfaces. The old
  immutable v2 bundles cannot negotiate the renamed bridge identifiers.
- Reconfigure every external plugin-hook receiver with its newly derived
  signing secret through a coordinated rotation while delivery is paused.
  Existing receivers retain an incompatible key until this is done.
- Before reusing a checkout, inspect its `prepare-commit-msg` hook and archive
  only hooks positively identified as old daemon-owned hooks. Never remove
  user-owned hooks. Let the Orvilo daemon install the current hook and verify
  both enabled and disabled co-author behavior before resuming work.
- Migration 597 converts persisted channel-media provenance markers and
  Desktop callback schemes. Verify existing attachments remain in descriptions
  after editing an issue, including an edit from a stale revision.

This checklist is a maintenance-window migration, not a rolling compatibility
contract. Do not let old clients or old plugin bundles operate on the new stack.

## Rollback procedure

Before accepting new writes, route traffic back and restart the untouched old
installation. After new writes have occurred, pause both sides and plan data
reconciliation before switching back; simply restarting the old database would
lose those writes. Do not run old binaries against the newly migrated database
without a separately tested downgrade.
