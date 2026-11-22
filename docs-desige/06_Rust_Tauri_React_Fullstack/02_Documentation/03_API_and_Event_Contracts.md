# API, Domain Data, and Event Contracts

## Public API (via Rust Gateway/BFF)

| Method | Path | owner | notes |
| --- | --- | --- | --- |
| GET | `/api/v1/catalog/products` | Catalog | pagination + filtering |
| GET | `/api/v1/catalog/products/{productId}` | Catalog | product metadata only |
| GET | `/api/v1/pricing/quotes` | Pricing | currency/channel/price validity |
| GET | `/api/v1/carts/{cartId}` | Cart | customer-bound ownership check |
| POST | `/api/v1/carts/{cartId}/items` | Cart | validate customer/session |
| POST | `/api/v1/checkouts` | Order | **Idempotency-Key** required, server computes quote |
| GET | `/api/v1/checkouts/{checkoutId}` | Order | saga status poll |
| GET | `/api/v1/orders/{orderId}` | Order | customer/role authorization |
| GET | `/api/v1/orders/{orderId}/shipments` | Fulfillment | read model / aggregation |

Admin endpoints should be separate role-protected namespaces, e.g. `/api/v1/admin/products`; all backend services validate authorization, not gateway alone.

## DTOs: boundaries

- `Product`: catalog-only fields (`id`, `sku`, `title`, `status`, `images`), **not current stock or price**.
- `PriceQuote`: `quote_id`, `product_id`, `unit_amount_minor`, `currency`, `expires_at`, `rule_version`.
- `StockReservation`: `reservation_id`, `product_id`, `quantity`, `expires_at`, `status`.
- `OrderLineSnapshot`: `product_id`, `sku`, `title`, `qty`, `unit_amount_minor`, `currency`, `tax_snapshot`.
- `Checkout`: `id`, `status`, `order_id?`, `failure_reason_code?`, `updated_at`.

Represent money using `int64` minor units for transport or decimal string with ISO currency, **never JS number for financial math**; when rendering JS display values, use locale formatting on safe converted nontransactional values or decimal library.

## Event envelope

```json
{
  "event_id": "019b0123-4bcd-7fff-8000-0123456789ab",
  "event_type": "order.confirmed.v1",
  "schema_version": 1,
  "aggregate_id": "019b0123-4bcd-7fff-8000-abcdef000012",
  "occurred_at": "2026-11-22T00:00:00Z",
  "correlation_id": "checkout-opaque-id",
  "trace_id": "trace-opaque-id",
  "payload": {
    "order_id": "019b0123-4bcd-7fff-8000-abcdef000012",
    "customer_id": "019b0123-4bcd-7fff-8000-abcdef000034",
    "currency": "THB",
    "grand_total_minor": "159900"
  }
}
```

Use an outbox row with unique event ID within `order_db` transaction; enqueue separately; consumer stores processed event ID and applies business updates in the same local DB transaction. Retain DLQ + retry alerts and delivery latency metrics.

### Key channels

- `catalog.product.updated.v1` => pricing validation / search projections
- `pricing.price.changed.v1` => cart price refresh / search
- `inventory.stock.changed.v1` => search / cart availability projections
- `inventory.reservation.expired.v1` => order saga compensation
- `order.confirmed.v1` => fulfillment / notifications
- `order.cancelled.v1` => compensation / notifications
- `payment.authorized.v1`, `payment.failed.v1` => order saga
- `payment.captured.v1` => order state / receipts
- `fulfillment.shipment.dispatched.v1` => order tracking / notifications

Contracts in the ZIP are **drafts**; contract tests, privacy review, and payment-provider integration are required before release.
