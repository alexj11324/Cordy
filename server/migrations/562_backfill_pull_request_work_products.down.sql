INSERT INTO issue_pull_request (
    issue_id, pull_request_id, linked_by_type, linked_by_id, linked_at,
    close_intent, reference_only
)
SELECT
    relation.issue_id,
    product.provider_record_id,
    'system',
    NULL,
    relation.attached_at,
    relation.close_intent,
    relation.relation_source = 'provider_reference'
FROM work_product_relation relation
JOIN work_product product ON product.id = relation.work_product_id
WHERE product.provider_record_type = 'github_pull_request'
  AND relation.issue_id IS NOT NULL
  AND relation.relation_source IN ('provider_discovery', 'provider_reference')
ON CONFLICT (issue_id, pull_request_id) DO UPDATE SET
    close_intent = EXCLUDED.close_intent,
    reference_only = EXCLUDED.reference_only;

INSERT INTO issue_vcs_pull_request (
    issue_id, pull_request_id, linked_by_type, linked_by_id, linked_at,
    close_intent, reference_only
)
SELECT
    relation.issue_id,
    product.provider_record_id,
    'system',
    NULL,
    relation.attached_at,
    relation.close_intent,
    relation.relation_source = 'provider_reference'
FROM work_product_relation relation
JOIN work_product product ON product.id = relation.work_product_id
WHERE product.provider_record_type = 'vcs_pull_request'
  AND relation.issue_id IS NOT NULL
  AND relation.relation_source IN ('provider_discovery', 'provider_reference')
ON CONFLICT (issue_id, pull_request_id) DO UPDATE SET
    close_intent = EXCLUDED.close_intent,
    reference_only = EXCLUDED.reference_only;

DELETE FROM work_product_relation
WHERE relation_source IN ('provider_discovery', 'provider_reference');

DELETE FROM work_product product
WHERE provider_record_type IN ('github_pull_request', 'vcs_pull_request')
  AND NOT EXISTS (
      SELECT 1 FROM work_product_relation relation
      WHERE relation.work_product_id = product.id
  );
