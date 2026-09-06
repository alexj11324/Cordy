DELETE FROM desktop_auth_handoff WHERE callback_protocol <> 'patchbay';
ALTER TABLE desktop_auth_handoff DROP CONSTRAINT desktop_auth_handoff_protocol_check;
ALTER TABLE desktop_auth_handoff ADD CONSTRAINT desktop_auth_handoff_protocol_check
    CHECK (callback_protocol = 'patchbay');
