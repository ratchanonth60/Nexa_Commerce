CREATE TABLE IF NOT EXISTS product (
    id uuid PRIMARY KEY,
    sku text UNIQUE NOT NULL,
    title text NOT NULL,
    status text NOT NULL CHECK (status IN ('draft','active','archived')),
    created_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO product (id, sku, title, status) VALUES
('019b0000-0000-7000-8000-000000000001','DEMO-TSHIRT','Nexa Demo T-shirt','active'),
('019b0000-0000-7000-8000-000000000002','DEMO-MUG','Nexa Demo Mug','active')
ON CONFLICT (id) DO NOTHING;
