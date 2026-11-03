-- A20d, D-IDEMPOTENCY-DEAD-CLAIM: a POST /api/v1/orders Idempotency-Key and the order it queued. Rust-only. The claim
-- writes (key_id, NULL); the order's insert sets order_id in the same transaction, so a NULL proves no order row was ever
-- written for the key. Every delete of an order row deletes its links first, in the same transaction (order ids are
-- reused). The start sweep reads it to settle a key its request died holding (rest_idempotency::sweep_dead_claims).
CREATE TABLE rust_idempotency_orders (key_id INTEGER PRIMARY KEY, order_id INTEGER);
CREATE INDEX index_rust_idempotency_orders_on_order_id ON rust_idempotency_orders (order_id);
