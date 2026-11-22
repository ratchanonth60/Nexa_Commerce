# Infrastructure & Delivery

## Logical deployment

- Public edge: CDN/static storefront app, HTTPS ingress/WAF and Axum Gateway/BFF.
- Internal runtime: Rust services running as individually deployable processes; private networking with least-privilege identities.
- Data: PostgreSQL service-owned databases and credentials; automated backups + PITR; Redis for disposable cache only.
- Messaging: RabbitMQ queues/exchanges, DLQs, retry policies, outbox publisher worker per service.
- OIDC: managed / self-hosted identity provider; do not build custom password logic into each service.
- Observability: `tracing`, OpenTelemetry Collector, Prometheus-compatible metrics, logs aggregator, distributed trace visualization.
- Secrets: vault/KMS/secret manager; never commit tokens, DB credentials, Tauri signing keys.

## Local dev

- Docker Compose launches Postgres/RabbitMQ/Redis; each Rust service runs with `cargo run -p ...`.
- Tauri prerequisites differ per OS: Rust toolchain, platform WebView support, C/C++ system dependencies where needed.
- Root frontend workspace uses pnpm; Tauri frontend Vite runs on a known fixed port. Backend can start first; frontend fetch should handle unavailable service errors.

## CI Pipeline (recommended)

1. Validate formatting/lint: `cargo fmt --check`, Clippy, `cargo test`, dependency security audit.
2. TypeScript checks: `pnpm lint`, `pnpm typecheck`, UI unit tests; browser e2e tests.
3. OpenAPI/JSON Schema validation + compatibility checks against consumer expectations.
4. Run PostgreSQL migrations, test rollback strategy, start service integration tests with ephemeral DB.
5. Build Rust Docker images, scan images and generate SBOM; sign images/provenance as supported.
6. Deploy service canaries independently; enable expanded traces, retries and alarms.
7. Build Tauri desktop for Windows/macOS/Linux on platform-specific runners; sign/notarize and publish signed updates.

## Production SLO draft (confirm with product requirements)

- API availability goal 99.9%; error budget may differ by route.
- Catalog read 95th percentile target under 300ms region-local, excluding network transit and external PSP.
- Checkout write API is asynchronous: return accepted state promptly and separately track saga completion latency.
- Use synthetic checkout monitors, idempotency duplication metrics, outbox lag, consumer lag, reservation expiration and payment discrepancy reports.

## Security baseline

- TLS everywhere, API rate limiting, WAF, input schema validation, SQL parameterization.
- AuthZ at gateway **and** backend; least-privilege DB accounts and service credentials.
- Separate PII from analytics; encryption and retention by regulation/business policy.
- Never log sensitive card data, tokens, session cookies, addresses or personal data in full.
- Backups restore-tested; migrations backwards compatible during rolling deployment.
