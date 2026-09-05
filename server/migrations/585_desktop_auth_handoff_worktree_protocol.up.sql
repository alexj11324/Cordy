-- Allow development Desktop worktrees to register distinct OS callback schemes.
-- Production remains `patchbay`; development uses a path-derived 16-hex
-- suffix that matches its unique app bundle identity.
ALTER TABLE desktop_auth_handoff DROP CONSTRAINT desktop_auth_handoff_protocol_check;
ALTER TABLE desktop_auth_handoff ADD CONSTRAINT desktop_auth_handoff_protocol_check
    CHECK (
        callback_protocol ~ '^(patchbay|patchbay-canary-[a-f0-9]{16})$'
    );
