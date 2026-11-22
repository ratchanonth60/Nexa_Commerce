# Proposed Monorepo Layout

```text
nexa-commerce/
├── apps/
│   ├── storefront-web/                 # React + TS + Vite (customer web)
│   ├── admin-web/                      # React + TS + Vite (optional)
│   └── desktop/                        # React + TS + Vite
│       ├── src/                        # shared UI composition
│       └── src-tauri/                  # Rust Tauri 2 + scoped commands
├── packages/
│   ├── ui/                             # design system, forms, layout
│   ├── contracts/                      # OpenAPI-generated types, Zod
│   ├── api-client/                     # interface, web and desktop adapters
│   └── config/                         # tsconfig/eslint/tailwind presets
├── services/
│   ├── gateway-bff/                    # Axum API Gateway / BFF
│   ├── customer-service/
│   ├── catalog-service/
│   ├── pricing-service/
│   ├── inventory-service/
│   ├── cart-service/
│   ├── order-service/
│   ├── payment-service/
│   └── fulfillment-service/
├── crates/
│   ├── common-types/                   # Rust common primitives, no domain models
│   ├── auth/                           # JWKS/JWT middleware / RBAC helpers
│   ├── messaging/                      # event envelope, outbox interfaces
│   └── telemetry/                      # tracing and metrics middleware
├── contracts/
│   ├── openapi/                        # versioned REST schema
│   └── events/                         # versioned events JSON schemas
├── infra/
│   ├── compose/                        # developer environment only
│   ├── migrations/                     # per-service pipeline dispatch
│   ├── k8s/                            # future; omit until needed
│   └── observability/
├── tests/
│   ├── e2e/                            # playwright / tauri driver later
│   └── contracts/                      # provider/consumer compatibility
├── Cargo.toml                          # Rust workspace (production repo)
├── pnpm-workspace.yaml                 # frontend workspace
├── package.json
└── README.md
```

## Workspace conventions

- Each service has `src/domain`, `src/application`, `src/adapters/{http,db,bus}`, `migrations/`, `tests/` when complexity warrants it.
- Each service uses one credentials set and DB migration directory; database ownership stays inside its bounded context.
- `crates/common-types` only has generic IDs and error envelopes; **do not share mutable domain entity structs** between services.
- TypeScript packages should not import Tauri from `packages/ui`; only `apps/desktop` or desktop transport adapter imports `@tauri-apps/api`.
- Version `GET /api/v1/...` routes; grow with explicit compatibility policies, not breaking silent field changes.
- `packages/contracts` is generated from OpenAPI; avoid manually redefining DTO fields in each app.
- Rust + TS CI stages can build independently; integration tests spin up PostgreSQL/RabbitMQ.

## Implementation scaffold

[`project/`](../../../project/README.md) contains the runnable Catalog API and both React clients. Run Cargo and pnpm commands from that directory.
