# Django Oscar — Core Database ER Diagrams

Source: https://github.com/django-oscar/django-oscar (master branch; inspected 2026-11-22)

These are **selected core Django Oscar models** and principal relations, not a full dump of all Django/Oscar tables. `AUTH_USER_MODEL` is configurable. `PK` and `FK` are shown explicitly in the image diagrams. Ellipses (`...`) abbreviate omitted columns. Nullable foreign keys are represented with optional cardinality. Django's ManyToMany creates join tables; some are automatic rather than declared as named model classes. The models shown can be customized using Oscar's forkable app/model mechanism.

## 1. ERD — Overview

```mermaid
erDiagram
    AUTH_USER_MODEL o|--o{ BASKET : owner
    AUTH_USER_MODEL o|--o{ ORDER : customer
    PRODUCT_CLASS o|--o{ PRODUCT : class
    PRODUCT o|--o{ PRODUCT : parent
    PRODUCT ||--o{ PRODUCT_CATEGORY : category_memberships
    CATEGORY ||--o{ PRODUCT_CATEGORY : product_memberships
    PRODUCT ||--o{ STOCK_RECORD : stockrecords
    PARTNER ||--o{ STOCK_RECORD : stockrecords
    BASKET ||--o{ BASKET_LINE : lines
    PRODUCT ||--o{ BASKET_LINE : lines
    STOCK_RECORD ||--o{ BASKET_LINE : lines
    BASKET o|--o{ ORDER : checkout_origin
    ORDER ||--o{ ORDER_LINE : lines
    PRODUCT o|--o{ ORDER_LINE : optional_product
    ORDER ||--o{ SOURCE : payment_sources
    SOURCE ||--o{ TRANSACTION : transactions
    ORDER ||--o{ SHIPPING_EVENT : shipping_events
    BASKET }o--o{ VOUCHER : many_to_many
    ORDER ||--o{ VOUCHER_APPLICATION : voucher_use
    VOUCHER ||--o{ VOUCHER_APPLICATION : applications
```

## 2. ERD — Catalogue & Inventory

```mermaid
erDiagram
    PRODUCT_CLASS o|--o{ PRODUCT : product_class_id
    PRODUCT o|--o{ PRODUCT : parent_id
    PRODUCT ||--o{ PRODUCT_CATEGORY : product_id
    CATEGORY ||--o{ PRODUCT_CATEGORY : category_id
    PRODUCT ||--o{ PRODUCT_IMAGE : product_id
    PRODUCT_CLASS o|--o{ PRODUCT_ATTRIBUTE : product_class_id
    PRODUCT ||--o{ PRODUCT_ATTRIBUTE_VALUE : product_id
    PRODUCT_ATTRIBUTE ||--o{ PRODUCT_ATTRIBUTE_VALUE : attribute_id
    PRODUCT ||--o{ STOCK_RECORD : product_id
    PARTNER ||--o{ STOCK_RECORD : partner_id

    PRODUCT_CLASS {
      int id PK
      string name
      string slug
      bool track_stock
    }
    CATEGORY {
      int id PK
      string name
      string slug
      string path
      int depth
    }
    PRODUCT {
      int id PK
      int product_class_id FK
      int parent_id FK
      string structure
      string title
      string upc
    }
    PRODUCT_CATEGORY {
      int id PK
      int product_id FK
      int category_id FK
    }
    PRODUCT_IMAGE {
      int id PK
      int product_id FK
      string original
      int display_order
    }
    PRODUCT_ATTRIBUTE {
      int id PK
      int product_class_id FK
      string code
      string type
    }
    PRODUCT_ATTRIBUTE_VALUE {
      int id PK
      int product_id FK
      int attribute_id FK
      string value_text
      int value_integer
    }
    PARTNER {
      int id PK
      string code
      string name
    }
    STOCK_RECORD {
      int id PK
      int product_id FK
      int partner_id FK
      string partner_sku
      decimal price
      int num_in_stock
      int num_allocated
    }
```

## 3. ERD — Baskets, Orders, Fulfilment

```mermaid
erDiagram
    AUTH_USER_MODEL o|--o{ USER_ADDRESS : addresses
    AUTH_USER_MODEL o|--o{ BASKET : owner
    AUTH_USER_MODEL o|--o{ ORDER : customer
    BASKET ||--o{ BASKET_LINE : lines
    PRODUCT ||--o{ BASKET_LINE : product
    STOCK_RECORD ||--o{ BASKET_LINE : stockrecord
    BASKET o|--o{ ORDER : source_basket
    SHIPPING_ADDRESS o|--o{ ORDER : shipping_address
    BILLING_ADDRESS o|--o{ ORDER : billing_address
    ORDER ||--o{ ORDER_LINE : lines
    PRODUCT o|--o{ ORDER_LINE : product
    PARTNER o|--o{ ORDER_LINE : partner
    STOCK_RECORD o|--o{ ORDER_LINE : stockrecord
    ORDER ||--o{ ORDER_DISCOUNT : discounts
    ORDER ||--o{ SHIPPING_EVENT : shipping_events
    SHIPPING_EVENT_TYPE ||--o{ SHIPPING_EVENT : type
    SHIPPING_EVENT ||--o{ SHIPPING_EVENT_QUANTITY : quantities
    ORDER_LINE ||--o{ SHIPPING_EVENT_QUANTITY : quantities

    AUTH_USER_MODEL {
      int id PK
    }
    USER_ADDRESS {
      int id PK
      int user_id FK
    }
    BASKET {
      int id PK
      int owner_id FK
      string status
    }
    BASKET_LINE {
      int id PK
      int basket_id FK
      int product_id FK
      int stockrecord_id FK
      int quantity
      decimal price_incl_tax
    }
    PRODUCT {
      int id PK
    }
    PARTNER {
      int id PK
    }
    STOCK_RECORD {
      int id PK
    }
    ORDER {
      int id PK
      string number
      int user_id FK
      int basket_id FK
      int shipping_address_id FK
      int billing_address_id FK
      decimal total_incl_tax
      string status
    }
    ORDER_LINE {
      int id PK
      int order_id FK
      int product_id FK
      int partner_id FK
      int stockrecord_id FK
      int quantity
      decimal line_price_incl_tax
    }
    SHIPPING_ADDRESS {
      int id PK
    }
    BILLING_ADDRESS {
      int id PK
    }
    ORDER_DISCOUNT {
      int id PK
      int order_id FK
      decimal amount
    }
    SHIPPING_EVENT {
      int id PK
      int order_id FK
      int event_type_id FK
    }
    SHIPPING_EVENT_TYPE {
      int id PK
      string code
    }
    SHIPPING_EVENT_QUANTITY {
      int id PK
      int event_id FK
      int line_id FK
      int quantity
    }
```

## 4. ERD — Payments, Vouchers, Offers

```mermaid
erDiagram
    ORDER ||--o{ SOURCE : sources
    SOURCE_TYPE ||--o{ SOURCE : type
    SOURCE ||--o{ TRANSACTION : transactions
    VOUCHER_SET o|--o{ VOUCHER : vouchers
    BASKET }o--o{ VOUCHER : many_to_many
    VOUCHER }o--o{ CONDITIONAL_OFFER : many_to_many
    VOUCHER ||--o{ VOUCHER_APPLICATION : redeemed
    ORDER ||--o{ VOUCHER_APPLICATION : recorded_on
    AUTH_USER_MODEL o|--o{ VOUCHER_APPLICATION : redeemed_by
    CONDITION ||--o{ CONDITIONAL_OFFER : conditions
    BENEFIT ||--o{ CONDITIONAL_OFFER : benefits
    RANGE o|--o{ CONDITION : condition_range
    RANGE o|--o{ BENEFIT : benefit_range
    RANGE ||--o{ RANGE_PRODUCT : products
    PRODUCT ||--o{ RANGE_PRODUCT : belongs_to

    ORDER {
      int id PK
    }
    SOURCE_TYPE {
      int id PK
      string code
      string name
    }
    SOURCE {
      int id PK
      int order_id FK
      int source_type_id FK
      decimal amount_debited
    }
    TRANSACTION {
      int id PK
      int source_id FK
      string txn_type
      decimal amount
    }
    VOUCHER_SET {
      int id PK
      string name
    }
    VOUCHER {
      int id PK
      int voucher_set_id FK
      string code
      string usage
    }
    BASKET {
      int id PK
    }
    VOUCHER_APPLICATION {
      int id PK
      int voucher_id FK
      int order_id FK
      int user_id FK
    }
    AUTH_USER_MODEL {
      int id PK
    }
    CONDITIONAL_OFFER {
      int id PK
      int condition_id FK
      int benefit_id FK
      string offer_type
    }
    CONDITION {
      int id PK
      int range_id FK
      string type
      decimal value
    }
    BENEFIT {
      int id PK
      int range_id FK
      string type
      decimal value
    }
    RANGE {
      int id PK
      string name
    }
    RANGE_PRODUCT {
      int id PK
      int range_id FK
      int product_id FK
    }
    PRODUCT {
      int id PK
    }
```

### Notes

- `catalogue.Product` includes standalone, parent and child product structures; `Product.parent_id` stores child-to-parent linkage.
- `partner.StockRecord` stores per-partner SKU, inventory and price details; the price is not a primary attribute of `catalogue.Product`.
- `basket.Line` and `order.Line` are two distinct models/tables. Checkout copies order line information; the default model has no `basket_line_id` FK on `order.Line`.
- `order.Order` can be placed by a guest (`user_id` nullable) and can retain a link to the original basket (`basket_id` nullable).
- `order.OrderDiscount.offer_id` and `voucher_id` are ordinary integer snapshot fields, **not** Django `ForeignKey` relations.
- `basket.Basket.vouchers` and `voucher.Voucher.offers` are M2M fields creating implicit join tables.

References: [catalogue](https://github.com/django-oscar/django-oscar/blob/master/src/oscar/apps/catalogue/abstract_models.py) · [partner](https://github.com/django-oscar/django-oscar/blob/master/src/oscar/apps/partner/abstract_models.py) · [basket](https://github.com/django-oscar/django-oscar/blob/master/src/oscar/apps/basket/abstract_models.py) · [order](https://github.com/django-oscar/django-oscar/blob/master/src/oscar/apps/order/abstract_models.py) · [payment](https://github.com/django-oscar/django-oscar/blob/master/src/oscar/apps/payment/abstract_models.py) · [voucher](https://github.com/django-oscar/django-oscar/blob/master/src/oscar/apps/voucher/abstract_models.py) · [offer](https://github.com/django-oscar/django-oscar/blob/master/src/oscar/apps/offer/abstract_models.py).
