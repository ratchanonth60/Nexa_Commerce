CREATE TABLE order_customer (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    issuer text NOT NULL,
    subject text NOT NULL,
    UNIQUE (issuer, subject)
);

CREATE TABLE checkout (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    customer_id uuid NOT NULL REFERENCES order_customer (id),
    idempotency_key text NOT NULL,
    cart_id uuid NOT NULL,
    cart_version bigint NOT NULL,
    sandbox_approve boolean NOT NULL,
    total_minor bigint NOT NULL CHECK (total_minor >= 0),
    status text NOT NULL DEFAULT 'created' CHECK (status IN ('created', 'confirmed', 'failed')),
    failure_reason_code text,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (customer_id, idempotency_key),
    CHECK ((status = 'failed') = (failure_reason_code IS NOT NULL))
);

CREATE UNIQUE INDEX checkout_one_active_order_per_cart
    ON checkout (cart_id) WHERE status <> 'failed';

CREATE INDEX checkout_ready ON checkout (next_attempt_at, id) WHERE status = 'created';

CREATE TABLE checkout_item (
    checkout_id uuid NOT NULL REFERENCES checkout (id),
    product_id uuid NOT NULL,
    sku text NOT NULL,
    title text NOT NULL,
    quantity integer NOT NULL CHECK (quantity BETWEEN 1 AND 9999),
    unit_amount_minor bigint NOT NULL CHECK (unit_amount_minor >= 0),
    rule_version integer NOT NULL CHECK (rule_version > 0),
    PRIMARY KEY (checkout_id, product_id)
);

CREATE TABLE order_outbox (
    event_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id uuid NOT NULL UNIQUE REFERENCES checkout (id),
    event_type text NOT NULL DEFAULT 'order.confirmed.v1',
    occurred_at timestamptz NOT NULL DEFAULT now(),
    payload jsonb NOT NULL
);
