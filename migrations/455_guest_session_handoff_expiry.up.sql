-- Bootstrap guests created for Desktop OAuth must stop authenticating when
-- their five-minute handoff attempt expires. Ordinary guest sessions retain a
-- NULL value and remain governed by their explicit logout/claim lifecycle.
ALTER TABLE guest_session
    ADD COLUMN handoff_expires_at TIMESTAMPTZ;
