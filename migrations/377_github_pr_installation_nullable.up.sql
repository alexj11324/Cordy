-- CORD-24: PR write-back at creation time. An agent (or a member) can attach
-- an existing GitHub PR to an issue via the API/CLI without any GitHub App
-- installation being present. The mirrored github_pull_request row therefore
-- may not have an installation yet; installation_id becomes nullable and is
-- backfilled by the webhook's ON CONFLICT upsert the first time a real
-- pull_request delivery for that repo lands.
ALTER TABLE github_pull_request
    ALTER COLUMN installation_id DROP NOT NULL;
