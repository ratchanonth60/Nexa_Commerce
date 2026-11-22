# Nexa Commerce catalog demo

The web storefront and the Tauri desktop app share a React product list and read the same public catalog endpoint. The browser uses HTTP directly. Tauri uses a Rust IPC command to call the API.

## Run locally

Install Rust stable, Node 24, pnpm 10, and Docker Compose. For the desktop app, install the [Tauri v2 platform prerequisites](https://v2.tauri.app/start/prerequisites/).

Run the following from `project/`.

```bash
docker compose --project-directory . -f infra/compose.yaml up -d --wait postgres
DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5432/catalog_db cargo run --locked -p catalog-service
```

The API runs migrations on startup and loads two demo products. Check it from another terminal.

```bash
curl --fail http://127.0.0.1:8082/api/v1/catalog/products
pnpm install
pnpm dev:web
```

Open http://127.0.0.1:5173. To launch the desktop app, keep the API running and use `pnpm dev:desktop` in another terminal.

## Verify

Run `cargo test --locked` to check limit validation and local CORS. Run `pnpm --filter @nexa/storefront-web build` and `pnpm --filter @nexa/desktop build` to type-check and build the clients. The starter CI workflow also checks the Tauri Rust crate and queries the catalog backed by seeded PostgreSQL.

The API accepts `limit` from 1 to 100 and defaults to 20. Both clients should display **Nexa Demo Mug** followed by **Nexa Demo T-shirt**. Select either title to read its SKU and status from `GET /api/v1/catalog/products/{productId}`, then select **Back to products** to return. The [implementation roadmap](../docs-desige/06_Rust_Tauri_React_Fullstack/02_Documentation/06_Implementation_Roadmap.md) covers cart, checkout, and later phases.

The [OpenAPI draft](contracts/nexa_gateway.openapi.yaml) describes catalog requests and responses. The [OrderConfirmed event schema](contracts/order.confirmed.v1.schema.json) describes the planned checkout event.
