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

## Rollback

Before accepting new writes, route traffic back and restart the untouched old
installation. After new writes have occurred, pause both sides and plan data
reconciliation before switching back; simply restarting the old database would
lose those writes. Do not run old binaries against the newly migrated database
without a separately tested downgrade.
