DROP TRIGGER trg_linear_work_product_url_outbox ON work_product;
DROP FUNCTION enqueue_linear_work_product_url_outbox();
DROP TRIGGER trg_linear_work_product_outbox ON work_product_relation;
DROP FUNCTION enqueue_linear_work_product_outbox();
DELETE FROM linear_sync_outbox WHERE event_type='attachment_deleted';
