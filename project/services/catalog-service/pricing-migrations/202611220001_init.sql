CREATE TABLE price (
    product_id uuid NOT NULL,
    currency text NOT NULL CHECK (currency ~ '^[A-Z]{3}$'),
    channel text NOT NULL,
    unit_amount_minor bigint NOT NULL CHECK (unit_amount_minor >= 0),
    rule_version integer NOT NULL DEFAULT 1 CHECK (rule_version > 0),
    PRIMARY KEY (product_id, currency, channel)
);

INSERT INTO price (product_id, currency, channel, unit_amount_minor) VALUES
('019b0000-0000-7000-8000-000000000001', 'THB', 'storefront', 39900),
('019b0000-0000-7000-8000-000000000002', 'THB', 'storefront', 24900);
