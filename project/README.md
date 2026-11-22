# Nexa Commerce local sandbox

The React storefront and Tauri desktop app share a catalog and cart UI. The Rust gateway serves the catalog, cart, checkout and shipment APIs. The browser uses the gateway’s same-origin session routes, and Tauri calls the gateway through Rust IPC.

## Run locally

Install Rust stable, Node 24, pnpm 10, and Docker Compose. For the desktop app, install the [Tauri v2 platform prerequisites](https://v2.tauri.app/start/prerequisites/).

Run the following from `project/`.

```bash
docker compose --project-directory . -f infra/compose.yaml up -d --wait postgres pricing-postgres inventory-postgres cart-postgres
DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5432/catalog_db \
PRICING_DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5433/pricing_db \
INVENTORY_DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5434/inventory_db \
CART_DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5435/cart_db \
  cargo run --locked -p catalog-service
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

## Pricing quotes

The demo gateway reads prices from the separate Pricing database. Request a quote for the seeded storefront price.

```bash
curl --fail 'http://127.0.0.1:8082/api/v1/pricing/quotes?product_id=019b0000-0000-7000-8000-000000000002&currency=THB&channel=storefront'
```

The quote is for display. Checkout will need server-side price validation before charging.

## Inventory availability

The demo gateway checks quantity against the separate Inventory database.

```bash
curl --fail 'http://127.0.0.1:8082/api/v1/inventory/availability?product_id=019b0000-0000-7000-8000-000000000002&quantity=1'
```

This read gives current availability. Checkout will need an inventory reservation before charging.

## Authenticated Cart API

Set `OIDC_ISSUER`, `OIDC_API_AUDIENCE`, and `OIDC_PUBLIC_KEY_FILE` together
before starting the API to enable cart routes. Supply the identity provider's
RSA public key in PEM format. The API accepts RS256 JWT access tokens with
the configured issuer, API audience, expiry, and the `at+jwt` token type.
Restart the gateway with the new public key after a signing-key change.

Send the access token in the `Authorization: Bearer` header. Create a cart
with `POST /api/v1/carts`, read it with `GET /api/v1/carts/{cartId}`, and
add a product with `POST /api/v1/carts/{cartId}/items`. Cart data lives in its
own PostgreSQL database. The API checks the token subject against cart
ownership and returns 403 for another customer's cart.

When the verifier is unset, cart routes return 503 and public product,
price, and availability reads are available.

## Customer sign-in with local Keycloak

Start the optional loopback-only identity provider from `project/`.

```bash
docker compose -f infra/compose.yaml --profile auth up -d keycloak
export OIDC_ISSUER=http://127.0.0.1:8080/realms/nexa
export OIDC_API_AUDIENCE=nexa-api
export OIDC_KEYCLOAK_ACCESS_TOKENS=true
export OIDC_PUBLIC_KEY_FILE="$(mktemp)"
curl --fail --silent --show-error "$OIDC_ISSUER/protocol/openid-connect/certs" |
  jq -er '.keys[] | select(.use == "sig" and .alg == "RS256") | .x5c[0]' |
  awk 'BEGIN { print "-----BEGIN CERTIFICATE-----" } { print } END { print "-----END CERTIFICATE-----" }' |
  openssl x509 -pubkey -noout > "$OIDC_PUBLIC_KEY_FILE"

export OIDC_WEB_CLIENT_ID=nexa-web
export OIDC_WEB_CLIENT_SECRET=nexa_local_web_only
export OIDC_WEB_AUTHORIZATION_URL="$OIDC_ISSUER/protocol/openid-connect/auth"
export OIDC_WEB_TOKEN_URL="$OIDC_ISSUER/protocol/openid-connect/token"
export OIDC_WEB_ORIGIN=http://127.0.0.1:5173
export OIDC_WEB_REDIRECT_URI="$OIDC_WEB_ORIGIN/auth/callback"
export OIDC_WEB_ALLOW_HTTP_LOOPBACK=true
```

Start the gateway with the database variables from **Run locally** in the
same shell. Open the storefront at `http://127.0.0.1:5173`, select **Sign in**,
and use `nexa-buyer` / `nexa_local_buyer_only`. Select a product and choose
**Add to cart**. The cart item count should increase. These credentials and
the HTTP loopback settings are for local development.

Vite proxies `/auth` and `/api` to the gateway. For a hosted storefront,
route both paths through the storefront's HTTPS origin. The browser keeps
an HttpOnly session cookie and supplies a CSRF token on writes. The Keycloak
verifier option retains issuer, audience, signature and expiry checks and
requires Keycloak's signed access-token type.

For desktop sign-in, start Tauri in a shell with the same `OIDC_ISSUER`,
`OIDC_API_AUDIENCE`, and `OIDC_KEYCLOAK_ACCESS_TOKENS` values.

```bash
export OIDC_DESKTOP_CLIENT_ID=nexa-desktop
pnpm dev:desktop
```

Select **Sign in** and complete the login in the system browser. The desktop
app receives the authorization code at `http://127.0.0.1:17842/oidc/callback`.
Use that exact URI when registering another public desktop client with PKCE.
The app holds the short-lived access token in Rust memory. Closing the app,
signing out, or reaching token expiry ends the desktop session.

## Local checkout sandbox

Start the Order and Payment databases alongside the four commerce databases.

```bash
docker compose -f infra/compose.yaml --profile checkout up -d --wait order-postgres payment-postgres
export CHECKOUT_SANDBOX=true
export ORDER_DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5436/order_db
export PAYMENT_DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5437/payment_db
```

Restart the gateway with those variables. Use an authenticated cart with a
current version and post to `/api/v1/checkouts`. Supply an `Idempotency-Key`
header of 12–128 letters, digits, dots, dashes, or underscores, and this payload.

```json
{
  "cart_id": "<owned-cart-uuid>",
  "cart_version": 1,
  "payment_method_token": "sandbox_approve"
}
```

Poll `GET /api/v1/checkouts/{checkoutId}` until `confirmed` or `failed`.
Use `GET /api/v1/orders/{orderId}` to read a confirmed order. A retry with the
same key and payload returns the original checkout. A confirmed cart must be
replaced with a new cart for a new order. Use `sandbox_decline` to exercise the
payment failure path and release stock. The local simulator takes these two
test tokens and omits shipping addresses. Confirmed orders and outbox records
are written together. The sandbox stores no card details.

## Local operations

Start the Fulfillment database with Checkout enabled.

```bash
docker compose -f infra/compose.yaml --profile operations up -d --wait fulfillment-postgres
export FULFILLMENT_DATABASE_URL=postgres://nexa:nexa_dev_only@127.0.0.1:5438/fulfillment_db
```

Restart the gateway, then run `VITE_CHECKOUT_SANDBOX=true pnpm dev:web`.
Sign in as the buyer, add a product, place a sandbox order, and refresh its
status to get the order ID. For desktop operations, sign in as
`nexa-operator` / `nexa_local_operator_only`. Open the order by ID in the
backoffice and advance it to packed, dispatched, then delivered. The
storefront displays the shipment and its notification history after refresh.

The operator role comes from the signed access token's `nexa_roles` array.
The local realm grants it only to the operator account. Keycloak imports the
realm when it first creates it. For an existing realm, apply the role mapper
and operator account from the realm file through Keycloak administration.
Dispatch persists a queue entry before adjusting allocated stock, and the worker
retries incomplete dispatches across gateway restarts. Notification history is
served from Fulfillment. Product search is backed by an active-only Catalog
GIN index. Inventory adjustments record the operator and previous and new
values in Inventory.

## Release checks

- Run the GitHub Actions commerce tests against PostgreSQL and build both clients. Exercise buyer sign-in, checkout, payment decline, operator stock adjustment, dispatch, and shipment visibility in staging.
- Keep `/metrics` and `/readyz` on the internal gateway listener. `/healthz` checks liveness and `/readyz` checks all configured databases. Verify the Prometheus scrape and connect alerts to the on-call destination.
- Start Prometheus with `prometheus --config.file=infra/observability/prometheus.yml`. The initial targets are 99.9% API availability and 95% of requests within 500 ms over a rolling 30-minute window. The alert rules exclude 4xx responses and require at least 100 requests for either ratio alert.
- Run `python3 infra/observability/load-smoke.py` against the local gateway. Record concurrency, request count, p95 latency, and errors. Use staging traffic and production SLOs to set release capacity thresholds.
- After placing and dispatching a sandbox order in the Compose stack, run `bash infra/backup-drill.sh`. The drill restores all seven databases into an isolated PostgreSQL container and compares counts plus order and fulfillment records. Production recovery also needs coordinated recovery points across databases and Identity Provider backups.
- Configure the production identity provider and key rotation. Use HTTPS for the storefront and keep sandbox credentials and payment tokens in local development. Verify the payment provider, webhook handling, shipping addresses and policies in staging before charging customers.

### Signed desktop releases

The **Build signed desktop update** workflow builds a Universal macOS archive.
Configure the `desktop-release` GitHub environment with
`TAURI_UPDATER_ENDPOINT` and `TAURI_UPDATER_PUBLIC_KEY`, plus secrets
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
Set `NEXA_GATEWAY_ORIGIN` to the hosted HTTPS gateway origin and configure
`OIDC_ISSUER`, `OIDC_DESKTOP_CLIENT_ID`, and `OIDC_API_AUDIENCE` for that release.
Use `OIDC_KEYCLOAK_ACCESS_TOKENS=true` for the compatible Keycloak token format.

The workflow uploads the signed update archive and signature as GitHub Actions
artifacts. The configured HTTPS manifest must describe the archive URL,
version, and signature. Verify an update from the previous version in staging.
Apple distribution also requires the applicable signing and notarization.
