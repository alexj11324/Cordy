INSERT INTO work_product (
    workspace_id, kind, provider, external_identity, external_url,
    provider_record_type, provider_record_id, created_at, updated_at
)
SELECT
    workspace_id,
    'pull_request',
    'github',
    repo_owner || '/' || repo_name || '#' || pr_number::text,
    html_url,
    'github_pull_request',
    id,
    created_at,
    updated_at
FROM github_pull_request
ON CONFLICT (workspace_id, provider, external_identity) DO UPDATE SET
    external_url = EXCLUDED.external_url,
    provider_record_type = EXCLUDED.provider_record_type,
    provider_record_id = EXCLUDED.provider_record_id,
    updated_at = GREATEST(work_product.updated_at, EXCLUDED.updated_at);

INSERT INTO work_product (
    workspace_id, kind, provider, external_identity, external_url,
    provider_record_type, provider_record_id, created_at, updated_at
)
SELECT
    workspace_id,
    'pull_request',
    provider,
    connection_id::text || ':' || repo_owner || '/' || repo_name || '#' || pr_number::text,
    html_url,
    'vcs_pull_request',
    id,
    created_at,
    updated_at
FROM vcs_pull_request
ON CONFLICT (workspace_id, provider, external_identity) DO UPDATE SET
    external_url = EXCLUDED.external_url,
    provider_record_type = EXCLUDED.provider_record_type,
    provider_record_id = EXCLUDED.provider_record_id,
    updated_at = GREATEST(work_product.updated_at, EXCLUDED.updated_at);

INSERT INTO work_product_relation (
    workspace_id, work_product_id, issue_id, relation_key, relation_source,
    attached_by_type, attached_by_id, attached_at, close_intent
)
SELECT
    pr.workspace_id,
    product.id,
    relation.issue_id,
    'provider:github_pull_request:' || pr.id::text || ':issue:' || relation.issue_id::text,
    CASE WHEN relation.reference_only THEN 'provider_reference' ELSE 'provider_discovery' END,
    'system',
    NULL,
    relation.linked_at,
    relation.close_intent
FROM issue_pull_request relation
JOIN github_pull_request pr ON pr.id = relation.pull_request_id
JOIN work_product product
  ON product.workspace_id = pr.workspace_id
 AND product.provider_record_type = 'github_pull_request'
 AND product.provider_record_id = pr.id
ON CONFLICT (work_product_id, relation_key) WHERE detached_at IS NULL DO UPDATE SET
    relation_source = EXCLUDED.relation_source,
    close_intent = EXCLUDED.close_intent,
    attached_at = LEAST(work_product_relation.attached_at, EXCLUDED.attached_at);

INSERT INTO work_product_relation (
    workspace_id, work_product_id, issue_id, relation_key, relation_source,
    attached_by_type, attached_by_id, attached_at, close_intent
)
SELECT
    pr.workspace_id,
    product.id,
    relation.issue_id,
    'provider:vcs_pull_request:' || pr.id::text || ':issue:' || relation.issue_id::text,
    CASE WHEN relation.reference_only THEN 'provider_reference' ELSE 'provider_discovery' END,
    'system',
    NULL,
    relation.linked_at,
    relation.close_intent
FROM issue_vcs_pull_request relation
JOIN vcs_pull_request pr ON pr.id = relation.pull_request_id
JOIN work_product product
  ON product.workspace_id = pr.workspace_id
 AND product.provider_record_type = 'vcs_pull_request'
 AND product.provider_record_id = pr.id
ON CONFLICT (work_product_id, relation_key) WHERE detached_at IS NULL DO UPDATE SET
    relation_source = EXCLUDED.relation_source,
    close_intent = EXCLUDED.close_intent,
    attached_at = LEAST(work_product_relation.attached_at, EXCLUDED.attached_at);
