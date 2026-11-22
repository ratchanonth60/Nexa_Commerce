CREATE TABLE cart (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_issuer text NOT NULL,
    owner_subject text NOT NULL,
    version bigint NOT NULL DEFAULT 0 CHECK (version >= 0)
);

CREATE TABLE cart_item (
    cart_id uuid NOT NULL REFERENCES cart (id) ON DELETE CASCADE,
    product_id uuid NOT NULL,
    quantity integer NOT NULL CHECK (quantity BETWEEN 1 AND 9999),
    PRIMARY KEY (cart_id, product_id)
);
