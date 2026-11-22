ALTER TABLE shipment ADD COLUMN dispatch_next_attempt_at timestamptz
    CHECK (dispatch_next_attempt_at IS NULL OR status = 'packed');

CREATE INDEX shipment_dispatch_due
    ON shipment (dispatch_next_attempt_at, order_id)
    WHERE dispatch_next_attempt_at IS NOT NULL;
