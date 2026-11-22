CREATE INDEX product_active_search_idx
    ON product USING gin (to_tsvector('simple', title || ' ' || sku))
    WHERE status = 'active';
