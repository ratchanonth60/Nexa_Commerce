# Nexa Commerce — Microservices (Proposed Target Design)

**Legacy reference (original Django Oscar framework):** https://github.com/django-oscar/django-oscar ; https://django-oscar.readthedocs.io/

> This is an architecture proposal and redesigned logical schema, **not** an exact extraction of Django Oscar model tables.

## 1. Architecture

```mermaid
flowchart TB
    Client[Web / Mobile / Admin] --> Gateway[API Gateway / BFF]
    Gateway --> Customer[Customer Service]
    Gateway --> Catalog[Catalog Service]
    Gateway --> Price[Pricing and Promotion Service]
    Gateway --> Cart[Cart Service]
    Gateway --> Order[Order and Checkout Service]
    Gateway --> Inv[Inventory Service]
    Gateway --> Payment[Payment Service]
    Gateway --> Fulfill[Fulfillment Service]
    Customer --> CustomerDB[(customer_db)]
    Catalog --> CatalogDB[(catalog_db)]
    Price --> PriceDB[(pricing_db)]
    Cart --> CartDB[(cart_db)]
    Order --> OrderDB[(order_db)]
    Inv --> InvDB[(inventory_db)]
    Payment --> PaymentDB[(payment_db)]
    Fulfill --> FulfillDB[(fulfillment_db)]
    Catalog -. ProductChanged .-> Bus[(Event Bus)]
    Inv -. StockChanged .-> Bus
    Order -. OrderConfirmed .-> Bus
    Payment -. PaymentCaptured .-> Bus
    Fulfill -. ShipmentDispatched .-> Bus
    Bus --> Search[Search Projection]
    Bus --> Notify[Notification Worker]
```

Each service owns its own PostgreSQL database (separate credentials/migration/backup). Cross-service identifiers are UUID references only, never foreign keys.

## 2. Database ERDs (per bounded context)

### Customer (customer_db)

```mermaid
erDiagram
    CUSTOMER ||--o{ CUSTOMER_ADDRESS : customer_id
    CUSTOMER {
        uuid id PK
        varchar oidc_subject
        varchar email
        varchar status
        datetime created_at
    }
    CUSTOMER_ADDRESS {
        uuid id PK
        uuid customer_id FK
        varchar name
        varchar line1
        varchar city
        varchar postcode
        varchar country_code
        boolean is_default
    }
```

### Catalog (catalog_db)

```mermaid
erDiagram
    PRODUCT_CLASS ||--o{ PRODUCT : product_class_id
    PRODUCT ||--o{ PRODUCT : parent_id
    CATEGORY ||--o{ CATEGORY : parent_id
    PRODUCT ||--o{ PRODUCT_CATEGORY : product_id
    CATEGORY ||--o{ PRODUCT_CATEGORY : category_id
    PRODUCT_CLASS ||--o{ PRODUCT_ATTRIBUTE : product_class_id
    PRODUCT ||--o{ ATTRIBUTE_VALUE : product_id
    PRODUCT_ATTRIBUTE ||--o{ ATTRIBUTE_VALUE : attribute_id
    PRODUCT ||--o{ PRODUCT_IMAGE : product_id
    PRODUCT_CLASS {
        uuid id PK
        varchar code
        varchar name
    }
    PRODUCT {
        uuid id PK
        uuid product_class_id FK
        uuid parent_id FK
        varchar sku
        varchar title
        varchar status
    }
    CATEGORY {
        uuid id PK
        uuid parent_id FK
        varchar name
        varchar slug
    }
    PRODUCT_CATEGORY {
        uuid product_id
        uuid category_id
    }
    PRODUCT_ATTRIBUTE {
        uuid id PK
        uuid product_class_id FK
        varchar code
        varchar data_type
    }
    ATTRIBUTE_VALUE {
        uuid id PK
        uuid product_id FK
        uuid attribute_id FK
        json value_json
    }
    PRODUCT_IMAGE {
        uuid id PK
        uuid product_id FK
        text image_url
        int sort_order
    }
```

### Pricing & Promotion (pricing_db)

```mermaid
erDiagram
    PROMOTION ||--o{ VOUCHER : promotion_id
    VOUCHER ||--o{ VOUCHER_REDEMPTION : voucher_id
    PRICE {
        uuid id PK
        uuid product_id
        varchar channel
        varchar currency
        decimal amount
        datetime valid_from
        datetime valid_to
    }
    PROMOTION {
        uuid id PK
        varchar name
        json rule_json
        json discount_json
        varchar status
    }
    VOUCHER {
        uuid id PK
        uuid promotion_id FK
        varchar code
        int usage_limit
        datetime valid_until
    }
    VOUCHER_REDEMPTION {
        uuid id PK
        uuid voucher_id FK
        uuid order_id
        uuid customer_id
        datetime redeemed_at
    }
```

### Inventory (inventory_db)

```mermaid
erDiagram
    PARTNER ||--o{ WAREHOUSE : partner_id
    WAREHOUSE ||--o{ STOCK_RECORD : warehouse_id
    STOCK_RECORD ||--o{ STOCK_RESERVATION : stock_record_id
    STOCK_RECORD ||--o{ STOCK_MOVEMENT : stock_record_id
    STOCK_RESERVATION ||--o{ STOCK_MOVEMENT : reservation_id
    PARTNER {
        uuid id PK
        varchar code
        varchar name
    }
    WAREHOUSE {
        uuid id PK
        uuid partner_id FK
        varchar code
        varchar name
    }
    STOCK_RECORD {
        uuid id PK
        uuid warehouse_id FK
        uuid product_id
        varchar partner_sku
        int on_hand
        int reserved
        int allocated
        int version
    }
    STOCK_RESERVATION {
        uuid id PK
        uuid stock_record_id FK
        uuid order_id
        int quantity
        varchar status
        datetime expires_at
    }
    STOCK_MOVEMENT {
        uuid id PK
        uuid stock_record_id FK
        uuid reservation_id FK
        int delta
        varchar reason
        datetime created_at
    }
```

### Cart (cart_db)

```mermaid
erDiagram
    CART ||--o{ CART_ITEM : cart_id
    CART {
        uuid id PK
        uuid customer_id
        varchar session_key
        varchar status
        datetime updated_at
    }
    CART_ITEM {
        uuid id PK
        uuid cart_id FK
        uuid product_id
        int quantity
        json selected_options
    }
```

### Order & Checkout (order_db)

```mermaid
erDiagram
    CHECKOUT_SESSION ||--o{ ORDERS : checkout_id
    ORDERS ||--o{ ORDER_ITEM : order_id
    ORDERS ||--o{ ORDER_ADDRESS : order_id
    ORDERS ||--o{ ORDER_DISCOUNT : order_id
    ORDERS ||--o{ ORDER_STATUS_HISTORY : order_id
    CHECKOUT_SESSION {
        uuid id PK
        uuid cart_id
        uuid customer_id
        varchar idempotency_key
        varchar state
        datetime expires_at
    }
    ORDERS {
        uuid id PK
        uuid checkout_id FK
        varchar order_number
        uuid customer_id
        varchar currency
        decimal grand_total
        varchar status
    }
    ORDER_ITEM {
        uuid id PK
        uuid order_id FK
        uuid product_id
        varchar sku_snapshot
        varchar title_snapshot
        int quantity
        decimal unit_price
        decimal tax_amount
    }
    ORDER_ADDRESS {
        uuid id PK
        uuid order_id FK
        varchar address_type
        json snapshot_json
    }
    ORDER_DISCOUNT {
        uuid id PK
        uuid order_id FK
        varchar voucher_code
        decimal amount
    }
    ORDER_STATUS_HISTORY {
        uuid id PK
        uuid order_id FK
        varchar status
        datetime changed_at
    }
```

### Payment (payment_db)

```mermaid
erDiagram
    PAYMENT_INTENT ||--o{ PAYMENT_TRANSACTION : payment_intent_id
    PAYMENT_INTENT ||--o{ REFUND : payment_intent_id
    PAYMENT_TRANSACTION ||--o{ REFUND : transaction_id
    PAYMENT_INTENT {
        uuid id PK
        uuid order_id
        varchar provider
        varchar provider_reference
        decimal amount
        varchar currency
        varchar status
        varchar idempotency_key
    }
    PAYMENT_TRANSACTION {
        uuid id PK
        uuid payment_intent_id FK
        varchar type
        decimal amount
        varchar provider_txn_id
        varchar status
    }
    REFUND {
        uuid id PK
        uuid payment_intent_id FK
        uuid transaction_id FK
        decimal amount
        varchar reason
        varchar status
    }
```

### Fulfillment (fulfillment_db)

```mermaid
erDiagram
    SHIPMENT ||--o{ SHIPMENT_ITEM : shipment_id
    SHIPMENT ||--o{ SHIPMENT_EVENT : shipment_id
    SHIPMENT {
        uuid id PK
        uuid order_id
        uuid warehouse_id
        varchar carrier
        varchar tracking_number
        varchar status
    }
    SHIPMENT_ITEM {
        uuid id PK
        uuid shipment_id FK
        uuid order_item_id
        int quantity
    }
    SHIPMENT_EVENT {
        uuid id PK
        uuid shipment_id FK
        varchar event_code
        datetime event_time
    }
```

## 3. Checkout / Payment Saga (orchestration)

```mermaid
sequenceDiagram
    autonumber
    actor Buyer
    participant API as API Gateway
    participant Order as Order and Checkout
    participant Price as Pricing and Promotion
    participant Stock as Inventory
    participant Pay as Payment
    participant Bus as Event Bus
    Buyer->>API: POST /checkouts (Idempotency-Key)
    API->>Order: Start checkout
    Order->>Price: Validate quote and voucher
    Price-->>Order: Signed quote with expiry
    Order->>Stock: Reserve items (order_id, TTL)
    alt Out of stock
        Stock-->>Order: Rejected
        Order-->>Buyer: 409 Stock unavailable
    else Reserved
        Stock-->>Order: reservation_ids
        Order->>Pay: Authorize(amount, order_id, idem key)
        alt Authorization declined
            Pay-->>Order: Declined
            Order->>Stock: Release reservations
            Order-->>Buyer: Payment declined
        else Authorized
            Pay-->>Order: Authorized
            Order->>Stock: Commit reservation to allocation
            alt Inventory commit fails
                Order->>Pay: Void authorization
                Order->>Stock: Release reservations
                Order-->>Buyer: Checkout failed; retry safely
            else Stock committed
                Order->>Order: Confirm order + immutable snapshots + outbox in one transaction
                Order->>Bus: OrderConfirmed (via outbox)
                Order-->>Buyer: 201 Order confirmed
            end
            Note over Pay,Bus: Capture according to fulfillment policy; refund/void on later failure
        end
    end
```

## 4. Communication & contracts

| Caller | Provider | Protocol | Purpose |
|---|---|---|---|
| Storefront | API Gateway/BFF | REST/HTTPS | UI composition, auth, throttling |
| Order | Pricing | REST/gRPC | Revalidate price/discount, return immutable quote |
| Order | Inventory | REST/gRPC | Atomic stock reservation, commit/release with TTL |
| Order | Payment | REST/gRPC | Create idempotent payment authorization |
| Order | Fulfillment | Events / API | Start picking/shipping after confirmed order |
| Any owner | Search and Notification | Event Bus | Materialized read model and customer messages |

## 5. Event catalog (examples)

`catalog.product.updated.v1`, `pricing.price.changed.v1`, `inventory.stock.changed.v1`, `inventory.reservation.expired.v1`, `order.confirmed.v1`, `order.cancelled.v1`, `payment.authorized.v1`, `payment.captured.v1`, `payment.failed.v1`, `fulfillment.shipment.dispatched.v1`.

Event envelope: `event_id`, `event_type`, `event_version`, `occurred_at`, `aggregate_id`, `correlation_id`, `payload`; avoid publishing sensitive customer address and raw payment data.

## 6. Design constraints

- Each service owns its schema, migrations, and outbox. Consumers deduplicate by `event_id` in inbox tables.
- Inventory reservation is atomic and expires automatically; `available = on_hand - reserved - allocated` and quantity cannot exceed available stock. Commit moves `reserved → allocated`; dispatch decrements both `on_hand` and `allocated`.
- Order stores immutable snapshots (SKU/title/unit price/tax/addresses) rather than relying on mutable Catalog and Customer rows.
- Payment webhooks must verify signatures and deduplicate provider transaction IDs. Never store raw card data.
- Handle compensation: release reservation after payment decline; void payment authorization on inventory/order failure; reverse stock allocation on local order-commit failure; refund per business policy after capture. An order is only externally CONFIRMED after allocation and durable order state succeed.
- Checkout `Idempotency-Key` prevents repeat order/payment on retries.
- Auth can be an OIDC identity provider; Customer service owns profile data and customer addresses.
- Search and Notification are projection/worker services, not authoritative databases.

## 7. Migration mapping: Django Oscar → Nexa Commerce

| Oscar app / model | Target | Important change |
|---|---|---|
| `catalogue.Product`, `Category`, `ProductClass`, attributes | Catalog | Own catalogue product metadata |
| `partner.Partner`, `StockRecord` | Inventory + Pricing | Split stock/partner from selling price, which Oscar typically couples through StockRecord/strategy |
| `basket.Basket`, `Line` | Cart | Basket references catalog product by UUID; price revalidated on checkout |
| `offer.*`, `voucher.*` | Pricing & Promotion | One rules/discount context initially; may split later |
| `order.Order`, `Line`, `OrderDiscount` | Order & Checkout | Immutable snapshot and orchestrated checkout |
| `payment.Source`, `Transaction` | Payment | Use provider reference, payment intent, captured/refund lifecycle |
| `order.ShippingEvent`, `shipping.*` | Fulfillment | Own shipment/shipment items/tracking |
| `address.*`, `customer.*`, `auth user` | Customer + OIDC IdP | Distinguish login identity from customer business profile |

## 8. Incremental migration from Django Oscar (strangler pattern)

1. Establish shared OIDC, gateway, correlation IDs, observability, and schema versioned events; keep Oscar as system of record.
2. Extract Catalog read APIs/search projections, then Cart; serve their APIs without cross-service writes.
3. Isolate Inventory with reservation API; stop order code from writing stock tables directly.
4. Extract Pricing & Promotion quote APIs; compare totals via shadow checks against Oscar.
5. Move Order/Checkout orchestration and Payment using outbox, webhooks and reconciliation, with order ID mapping.
6. Move Fulfillment and Customer data by ownership; retire corresponding Oscar paths only after dual-read validation.

**Warning:** Avoid bidirectional dual-write to Oscar and new services. Pick a single owner per entity and migrate through change-data-capture or outbox to avoid divergent records.
