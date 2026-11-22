CREATE TABLE stock_record (
    product_id uuid PRIMARY KEY,
    on_hand integer NOT NULL CHECK (on_hand >= 0),
    reserved integer NOT NULL CHECK (reserved BETWEEN 0 AND on_hand)
);

INSERT INTO stock_record (product_id, on_hand, reserved) VALUES
('019b0000-0000-7000-8000-000000000001', 0, 0),
('019b0000-0000-7000-8000-000000000002', 15, 3);
