CREATE TYPE shipment_status AS ENUM ('pending', 'packed', 'dispatched', 'delivered');

CREATE TABLE shipment (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id uuid NOT NULL UNIQUE,
    status shipment_status NOT NULL DEFAULT 'pending'
);

CREATE TABLE fulfillment_inbox (
    event_id uuid PRIMARY KEY,
    order_id uuid NOT NULL REFERENCES shipment (order_id),
    consumed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE fulfillment_notification_outbox (
    event_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    shipment_id uuid NOT NULL REFERENCES shipment (id),
    event_type text NOT NULL CHECK (event_type IN (
        'fulfillment.shipment.dispatched.v1', 'fulfillment.shipment.delivered.v1'
    )),
    occurred_at timestamptz NOT NULL DEFAULT now(),
    payload jsonb NOT NULL,
    UNIQUE (shipment_id, event_type)
);
