ALTER TYPE checkout_reservation_status ADD VALUE 'dispatched';

CREATE TABLE stock_adjustment (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id uuid NOT NULL REFERENCES stock_record(product_id),
    before_on_hand integer NOT NULL,
    after_on_hand integer NOT NULL,
    actor_issuer text NOT NULL,
    actor_subject text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);
