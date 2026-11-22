ALTER TABLE order_outbox ADD COLUMN fulfilled_at timestamptz;
CREATE INDEX order_outbox_fulfillment_pending
    ON order_outbox (occurred_at, event_id) WHERE fulfilled_at IS NULL;
