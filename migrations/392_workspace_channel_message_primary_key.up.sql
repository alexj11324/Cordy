ALTER TABLE workspace_channel_message
    ADD CONSTRAINT workspace_channel_message_pkey
    PRIMARY KEY USING INDEX workspace_channel_message_id_uidx;
