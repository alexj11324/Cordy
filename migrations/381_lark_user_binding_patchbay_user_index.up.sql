CREATE INDEX CONCURRENTLY idx_lark_user_binding_patchbay_user
    ON lark_user_binding(patchbay_user_id, workspace_id);
