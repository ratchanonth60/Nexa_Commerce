.DEFAULT_GOAL := help
SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c

ROOT := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
PROJECT := $(ROOT)project

# Optional local overrides use KEY=value syntax.
-include $(PROJECT)/.env
export

DOCKER ?= $(if $(wildcard /Applications/Docker.app/Contents/Resources/bin/docker),/Applications/Docker.app/Contents/Resources/bin/docker,docker)
PNPM ?= pnpm
COMPOSE = "$(DOCKER)" compose --project-directory "$(PROJECT)" -f "$(PROJECT)/infra/compose.yaml" --profile auth --profile checkout --profile operations
DATABASE_SERVICES := postgres pricing-postgres inventory-postgres cart-postgres order-postgres payment-postgres fulfillment-postgres

export DATABASE_URL ?= postgres://nexa:nexa_dev_only@127.0.0.1:5432/catalog_db
export PRICING_DATABASE_URL ?= postgres://nexa:nexa_dev_only@127.0.0.1:5433/pricing_db
export INVENTORY_DATABASE_URL ?= postgres://nexa:nexa_dev_only@127.0.0.1:5434/inventory_db
export CART_DATABASE_URL ?= postgres://nexa:nexa_dev_only@127.0.0.1:5435/cart_db
export ORDER_DATABASE_URL ?= postgres://nexa:nexa_dev_only@127.0.0.1:5436/order_db
export PAYMENT_DATABASE_URL ?= postgres://nexa:nexa_dev_only@127.0.0.1:5437/payment_db
export FULFILLMENT_DATABASE_URL ?= postgres://nexa:nexa_dev_only@127.0.0.1:5438/fulfillment_db
export CHECKOUT_SANDBOX ?= true
export VITE_CHECKOUT_SANDBOX ?= true
export RUST_LOG ?= info
export COMPOSE_PROJECT_NAME ?= nexa
GATEWAY_URL ?= http://127.0.0.1:8082
LOAD_SECONDS ?= 3
LOAD_WORKERS ?= 4
LOAD_PORT ?= 8082

.PHONY: help install up db-up stop down status logs api api-auth web desktop health test test-db check build backup-drill load docs

help: ## Show available commands
	@awk 'BEGIN {FS = ":.*## "} /^[a-z-]+:.*## / {printf "  make %-16s %s\n", $$1, $$2}' "$(ROOT)Makefile"

install: ## Install frontend dependencies from lockfile
	cd "$(PROJECT)" && $(PNPM) install --frozen-lockfile

up: ## Start seven databases and local Keycloak
	$(COMPOSE) up -d --wait $(DATABASE_SERVICES) keycloak

db-up: ## Start seven databases
	$(COMPOSE) up -d --wait $(DATABASE_SERVICES)

stop: ## Stop Compose services and keep containers and data
	$(COMPOSE) stop

down: ## Remove Compose containers and networks, keeping database volumes
	$(COMPOSE) down

status: ## Show Compose service status
	$(COMPOSE) ps

logs: ## Follow Compose logs, optionally SERVICE=keycloak
	$(COMPOSE) logs --follow --tail 100 $(SERVICE)

api: ## Run gateway in foreground with current authentication settings
	cd "$(PROJECT)" && cargo run --locked -p catalog-service

api-auth: ## Run gateway with local Keycloak login and sandbox checkout
	@key_file=$$(mktemp); trap 'rm -f -- "$$key_file"' EXIT; \
	export OIDC_ISSUER="$${OIDC_ISSUER:-http://127.0.0.1:8080/realms/nexa}"; \
	export OIDC_API_AUDIENCE="$${OIDC_API_AUDIENCE:-nexa-api}"; \
	export OIDC_KEYCLOAK_ACCESS_TOKENS=true; \
	export OIDC_PUBLIC_KEY_FILE="$$key_file"; \
	curl --fail --silent --show-error "$$OIDC_ISSUER/protocol/openid-connect/certs" | \
		jq -er '.keys[] | select(.use == "sig" and .alg == "RS256") | .x5c[0]' | \
		awk 'BEGIN {print "-----BEGIN CERTIFICATE-----"} {print} END {print "-----END CERTIFICATE-----"}' | \
		openssl x509 -pubkey -noout > "$$key_file"; \
	export OIDC_WEB_CLIENT_ID="$${OIDC_WEB_CLIENT_ID:-nexa-web}"; \
	export OIDC_WEB_CLIENT_SECRET="$${OIDC_WEB_CLIENT_SECRET:-nexa_local_web_only}"; \
	export OIDC_WEB_AUTHORIZATION_URL="$$OIDC_ISSUER/protocol/openid-connect/auth"; \
	export OIDC_WEB_TOKEN_URL="$$OIDC_ISSUER/protocol/openid-connect/token"; \
	export OIDC_WEB_ORIGIN=http://127.0.0.1:5173; \
	export OIDC_WEB_REDIRECT_URI="$$OIDC_WEB_ORIGIN/auth/callback"; \
	export OIDC_WEB_ALLOW_HTTP_LOOPBACK=true; \
	cd "$(PROJECT)" && cargo run --locked -p catalog-service

web: ## Run storefront in foreground
	cd "$(PROJECT)" && $(PNPM) dev:web

desktop: ## Run Tauri in foreground with local Keycloak settings
	cd "$(PROJECT)" && \
		OIDC_ISSUER="$${OIDC_ISSUER:-http://127.0.0.1:8080/realms/nexa}" \
		OIDC_API_AUDIENCE="$${OIDC_API_AUDIENCE:-nexa-api}" \
		OIDC_DESKTOP_CLIENT_ID="$${OIDC_DESKTOP_CLIENT_ID:-nexa-desktop}" \
		OIDC_KEYCLOAK_ACCESS_TOKENS=true $(PNPM) dev:desktop

health: ## Check readiness and catalog API
	curl --fail --silent --show-error "$(GATEWAY_URL)/readyz"
	@printf '\n'
	curl --fail --silent --show-error "$(GATEWAY_URL)/api/v1/catalog/products"
	@printf '\n'

test: ## Run Rust unit tests
	cd "$(PROJECT)" && cargo test --locked

test-db: ## Run PostgreSQL tests using isolated temporary databases
	cd "$(PROJECT)" && cargo test --locked -- --ignored

check: ## Check Rust formatting, Clippy and Tauri compilation
	cd "$(PROJECT)" && cargo fmt --check
	cd "$(PROJECT)" && cargo clippy --locked --all-targets -- -D warnings
	cd "$(PROJECT)" && cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml

build: ## Build gateway and both frontend clients
	cd "$(PROJECT)" && cargo build --locked
	cd "$(PROJECT)" && $(PNPM) --filter @nexa/storefront-web --filter @nexa/desktop build

backup-drill: ## Restore and compare seven databases with order/shipment fixtures
	PATH="$(dir $(DOCKER)):$$PATH" bash "$(PROJECT)/infra/backup-drill.sh"

load: ## Run gateway load smoke test
	python3 "$(PROJECT)/infra/observability/load-smoke.py" --seconds "$(LOAD_SECONDS)" --workers "$(LOAD_WORKERS)" --port "$(LOAD_PORT)"

docs: ## Print API documentation URL and verify it is available
	@printf '%s/docs\n' "$(GATEWAY_URL)"
	@curl --fail --silent --show-error --output /dev/null "$(GATEWAY_URL)/docs"
