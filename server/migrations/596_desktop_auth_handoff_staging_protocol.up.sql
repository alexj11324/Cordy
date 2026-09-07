-- Allow internal Desktop staging to register a distinct OS callback scheme
-- from Canary and production. Production remains `patchbay`; Canary remains
-- `patchbay-canary-<16 hex>`; staging uses `patchbay-staging-<16 hex>`.
-- Numbered 596 so a merge with main does not collide with
-- 591–595 automation trigger and memory migrations.
ALTER TABLE desktop_auth_handoff DROP CONSTRAINT desktop_auth_handoff_protocol_check;
ALTER TABLE desktop_auth_handoff ADD CONSTRAINT desktop_auth_handoff_protocol_check
    CHECK (
        callback_protocol ~ '^(patchbay|patchbay-(canary|staging)-[a-f0-9]{16})$'
    );
