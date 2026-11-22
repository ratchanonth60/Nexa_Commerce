# Implementation Roadmap — Suggested sequence

| Phase | Scope | Acceptance evidence |
| --- | --- | --- |
| 0 — Foundation | Monorepo, Rust Axum service skeleton, Vite+React UI, Tauri demo, PostgreSQL local, OpenAPI, CI | Web and Tauri show the same catalog data from one gateway contract |
| 1 — Commerce read & cart | Catalog, Pricing read quotes, Inventory availability, Cart and customer login | Authenticated web+desktop can browse, quote price and create cart; no cross-DB SQL joins |
| 2 — Checkout vertical slice | Order saga, inventory reservations, Payment PSP sandbox, outbox, idempotency | Automated checkout, payment failure compensation, no double order under retries |
| 3 — Operations | Fulfillment, inventory administration, Tauri backoffice, shipments, notifications, search | Operator role boundaries, shipment status visible end-to-end |
| 4 — Production hardening | load testing, audit logging, backup/restore, signed updater, observability, security review | Release checklist, recovery drills, monitored SLOs |

## MVP suggestion

Start with a **thin vertical slice** instead of implementing all eight microservices at once. In the first demo, build one web screen + one Tauri screen that load products using the same contract, then add cart and end-to-end checkout sandbox. It is acceptable to deploy domain modules together at first while maintaining clear data ownership and APIs.

## Decisions needed before production

1. Desktop app role: POS/cashier, warehouse, administrator, or customer?
2. Single seller or marketplace/multiple tenants? Partner model and tenant scoping differ.
3. Payment provider, currencies, refunds, taxes and cash-on-delivery.
4. Inventory semantics: single warehouse vs multiwarehouse / dropship.
5. Online-only or offline checkout / POS reconciliation requirement.
6. Supported desktop OS and distribution method.
7. Expected traffic, order throughput, SLO, compliance and delivery capacity.

These decisions change implementation details but not the high-level stack.
