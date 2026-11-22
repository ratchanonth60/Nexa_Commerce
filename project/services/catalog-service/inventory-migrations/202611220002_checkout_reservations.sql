ALTER TABLE stock_record
    ADD COLUMN allocated integer NOT NULL DEFAULT 0
    CHECK (allocated BETWEEN 0 AND on_hand - reserved);

CREATE TYPE checkout_reservation_status AS ENUM ('reserved', 'allocated', 'released', 'expired');

CREATE TABLE checkout_reservation (
    checkout_id uuid PRIMARY KEY,
    status checkout_reservation_status NOT NULL DEFAULT 'reserved',
    expires_at timestamptz NOT NULL DEFAULT (clock_timestamp() + interval '10 minutes')
);

CREATE INDEX checkout_reservation_due_idx
    ON checkout_reservation (expires_at, checkout_id)
    WHERE status = 'reserved';

CREATE TABLE checkout_reservation_item (
    checkout_id uuid NOT NULL REFERENCES checkout_reservation (checkout_id),
    product_id uuid NOT NULL REFERENCES stock_record (product_id),
    quantity integer NOT NULL CHECK (quantity BETWEEN 1 AND 9999),
    PRIMARY KEY (checkout_id, product_id)
);
