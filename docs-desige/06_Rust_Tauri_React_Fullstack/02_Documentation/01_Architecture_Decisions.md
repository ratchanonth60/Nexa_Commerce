# Architecture Decisions — Nexa Commerce Rust Fullstack

## ADR-001 — Rust + Axum for HTTP service runtime

Use `axum` + `tokio` and `tower-http` to implement REST/JSON APIs. Use `diesel-async` for pooled PostgreSQL access and Diesel migrations for schema changes. Separate `domain`, `application`, `ports`, `adapters`, `api` in larger services; don't force a big abstraction on tiny service starters. Use `tracing` + OpenTelemetry for visibility. **No Django runtime in the target system**; Django Oscar remains legacy domain reference.

Alternative `actix-web` is valid; Axum chosen for simple tower ecosystem integration. Protobuf/gRPC is optional for stable high-volume interservice contracts and not needed for MVP.

## ADR-002 — Tauri is a client app, not the backend microservice host

The `src-tauri` Rust executable provides OS integrations, authentication/token handling, local file operations, cache (optional) and HTTPS requests to the gateway. The 8 Rust services run in containers/cloud. Do not embed business services or credentials in the desktop binary; local PC won't require 8 database connections.

Suggested client roles: `storefront-web` for customers, `desktop` for internal backoffice / store staff. The desktop UI may be changed to a customer-facing app without changing service boundaries.

## ADR-003 — Shared React/TypeScript packages with separate transports

`packages/ui`: reusable components, form widgets, tokens.
`packages/contracts`: DTO/types, runtime schema validation, generated OpenAPI declarations.
`packages/api-client`: `CommerceApi` interface and implementations.

Web adapter uses `fetch` to the BFF/gateway, with credentials via secure SameSite HttpOnly cookies and CSRF protection. Desktop adapter uses Tauri `invoke()` for privileged flows; backend HTTPS calls can run in Rust and keep tokens out of JS. React hooks should not know which transport is active. Token storage for desktop uses native secure credentials, not localStorage.

## ADR-004 — Database per bounded context

Preserve ownership of `customer_db`, `catalog_db`, `pricing_db`, `inventory_db`, `cart_db`, `order_db`, `payment_db`, `fulfillment_db`. Every service alone owns its migrations and writes. Query cross-service data through public APIs, snapshots, or read projections. Avoid shared ORM models and cross-service SQL joins.

Existing Oscar StockRecord pricing logic is intentionally split. Catalog owns product metadata; Pricing owns effective price; Inventory owns stock and reservations.

## ADR-005 — Checkout as stateful Saga

Order service owns saga state and orchestrates price quote -> stock reservation -> payment authorization -> order commit -> downstream fulfillment. Operations retry on transient failures with bounded backoff. Compensation should void payment authorization and release inventory reservation. Maintain explicit state transitions (e.g. `CREATED`, `RESERVED`, `AUTHORIZED`, `CONFIRMED`, `FAILED`, `COMPENSATING`) and retry expiry policies. Use idempotency both on HTTP checkout and PSP calls.

## ADR-006 — Event-driven integration, outbox, observability

An event is written atomically in service's own DB transaction and published asynchronously by an outbox publisher. Consumers deduplicate by `event_id`. Include `event_type`, `schema_version`, `aggregate_id`, `occurred_at`, `trace_id`, and `payload`. Broker at-least-once delivery means duplicate processing is normal; no exactly-once assumption.

## ADR-007 — Protect checkout & money

Amounts use `NUMERIC` / integer minor units and ISO currency, never floats. No storage of raw card data or CVC. PSP handles cardholder data via hosted/tokenized flow. Server revalidates prices and stock; client totals are untrusted. Cross-currency rates, taxes and shipping are explicitly versioned domain decisions to implement.

## ADR-008 — Pragmatic rollout

Build one vertical slice (Catalog -> Cart -> Checkout -> Payment sandbox) first. Deploy grouped modules or a smaller number of processes if operations staffing is limited; ownership boundaries matter more than blindly deploying 8 services from day one.
