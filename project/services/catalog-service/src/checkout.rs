use std::{env, io, time::Duration};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Uuid, FromRow, PgPool, Postgres, Transaction};
use tracing::error;

use crate::{auth::Customer, cart, inventory, payment, AppState};

#[derive(Clone)]
pub(crate) struct CheckoutDatabases {
    order: PgPool,
    payment: PgPool,
}

impl CheckoutDatabases {
    pub(crate) async fn ping(&self) -> Result<(), sqlx::Error> {
        tokio::try_join!(crate::ping(&self.order), crate::ping(&self.payment))?;
        Ok(())
    }

    pub(crate) fn order_pool(&self) -> &PgPool {
        &self.order
    }

    pub(crate) async fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let order_url = env::var("ORDER_DATABASE_URL").ok();
        let payment_url = env::var("PAYMENT_DATABASE_URL").ok();
        let sandbox = env::var("CHECKOUT_SANDBOX").ok();
        match (sandbox.as_deref(), order_url, payment_url) {
            (None | Some("false"), None, None) => Ok(None),
            (Some("true"), Some(order), Some(payment)) => {
                let order = PgPool::connect(&order).await?;
                let payment = PgPool::connect(&payment).await?;
                sqlx::migrate!("./order-migrations").run(&order).await?;
                sqlx::migrate!("./payment-migrations").run(&payment).await?;
                Ok(Some(Self { order, payment }))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "set CHECKOUT_SANDBOX=true, ORDER_DATABASE_URL and PAYMENT_DATABASE_URL together",
            )
            .into()),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct CheckoutRequest {
    cart_id: Uuid,
    cart_version: i64,
    payment_method_token: String,
    shipping_address_id: Option<Uuid>,
}

#[derive(FromRow)]
struct Checkout {
    id: Uuid,
    cart_id: Uuid,
    cart_version: i64,
    sandbox_approve: bool,
    status: String,
    failure_reason_code: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct CheckoutStatus {
    id: Uuid,
    status: String,
    order_id: Option<Uuid>,
    failure_reason_code: Option<String>,
}

impl From<Checkout> for CheckoutStatus {
    fn from(row: Checkout) -> Self {
        Self {
            id: row.id,
            order_id: (row.status == "confirmed").then_some(row.id),
            status: row.status,
            failure_reason_code: row.failure_reason_code,
        }
    }
}

#[derive(Serialize, FromRow)]
struct CheckoutLine {
    product_id: Uuid,
    sku: String,
    title: String,
    quantity: i32,
    unit_amount_minor: i64,
    rule_version: i32,
}

#[derive(Serialize)]
pub(crate) struct Order {
    id: Uuid,
    status: &'static str,
    currency: &'static str,
    total_minor: String,
    items: Vec<OrderLine>,
}

#[derive(Serialize)]
struct OrderLine {
    product_id: Uuid,
    sku: String,
    title: String,
    quantity: i32,
    unit_amount_minor: String,
    rule_version: i32,
}

pub(crate) async fn create(
    customer: Customer,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CheckoutRequest>,
) -> Result<(StatusCode, Json<CheckoutStatus>), StatusCode> {
    let databases = state
        .checkout
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let key = idempotency_key(&headers)?;
    let sandbox_approve = match request.payment_method_token.as_str() {
        "sandbox_approve" => true,
        "sandbox_decline" => false,
        _ => return Err(StatusCode::UNPROCESSABLE_ENTITY),
    };
    if request.cart_version < 0 || request.shipping_address_id.is_some() {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let customer_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO order_customer (issuer, subject) VALUES ($1, $2)
         ON CONFLICT (issuer, subject) DO UPDATE SET issuer = EXCLUDED.issuer RETURNING id",
    )
    .bind(&customer.issuer)
    .bind(&customer.subject)
    .fetch_one(&databases.order)
    .await
    .map_err(db_error)?;

    if let Some(existing) = by_key(&databases.order, customer_id, key)
        .await
        .map_err(db_error)?
    {
        return replay(existing, &request, sandbox_approve);
    }

    let (lines, total) = snapshot(&state, &customer, &request).await?;
    let mut tx = databases.order.begin().await.map_err(db_error)?;
    let checkout = sqlx::query_as::<_, Checkout>(
        "INSERT INTO checkout (customer_id, idempotency_key, cart_id, cart_version, sandbox_approve, total_minor)
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING
         RETURNING id, cart_id, cart_version, sandbox_approve, status, failure_reason_code",
    )
    .bind(customer_id)
    .bind(key)
    .bind(request.cart_id)
    .bind(request.cart_version)
    .bind(sandbox_approve)
    .bind(total)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_error)?;

    let Some(checkout) = checkout else {
        tx.rollback().await.map_err(db_error)?;
        let existing = by_key(&databases.order, customer_id, key)
            .await
            .map_err(db_error)?
            .ok_or(StatusCode::CONFLICT)?;
        return replay(existing, &request, sandbox_approve);
    };

    for line in lines {
        sqlx::query(
            "INSERT INTO checkout_item (checkout_id, product_id, sku, title, quantity, unit_amount_minor, rule_version)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(checkout.id)
        .bind(line.product_id)
        .bind(line.sku)
        .bind(line.title)
        .bind(line.quantity)
        .bind(line.unit_amount_minor)
        .bind(line.rule_version)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    }
    tx.commit().await.map_err(db_error)?;
    Ok((StatusCode::ACCEPTED, Json(checkout.into())))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, StatusCode> {
    let mut keys = headers.get_all("idempotency-key").iter();
    let key = keys
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;
    if keys.next().is_some()
        || !(12..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(key)
}

async fn by_key(
    pool: &PgPool,
    customer_id: Uuid,
    key: &str,
) -> Result<Option<Checkout>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, cart_id, cart_version, sandbox_approve, status, failure_reason_code
         FROM checkout WHERE customer_id = $1 AND idempotency_key = $2",
    )
    .bind(customer_id)
    .bind(key)
    .fetch_optional(pool)
    .await
}

fn replay(
    checkout: Checkout,
    request: &CheckoutRequest,
    sandbox_approve: bool,
) -> Result<(StatusCode, Json<CheckoutStatus>), StatusCode> {
    if checkout.cart_id != request.cart_id
        || checkout.cart_version != request.cart_version
        || checkout.sandbox_approve != sandbox_approve
    {
        return Err(StatusCode::CONFLICT);
    }
    Ok((StatusCode::ACCEPTED, Json(checkout.into())))
}

async fn snapshot(
    state: &AppState,
    customer: &Customer,
    request: &CheckoutRequest,
) -> Result<(Vec<CheckoutLine>, i64), StatusCode> {
    let (version, items) =
        cart::checkout_snapshot(&state.cart_db, request.cart_id, customer).await?;
    if version != request.cart_version || items.is_empty() {
        return Err(StatusCode::CONFLICT);
    }
    let ids = items.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let products = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, sku, title FROM product WHERE status = 'active' AND id = ANY($1) ORDER BY id",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await
    .map_err(db_error)?;
    let prices = sqlx::query_as::<_, (Uuid, i64, i32)>(
        "SELECT product_id, unit_amount_minor, rule_version FROM price
         WHERE product_id = ANY($1) AND currency = 'THB' AND channel = 'storefront'
         ORDER BY product_id",
    )
    .bind(&ids)
    .fetch_all(&state.pricing_db)
    .await
    .map_err(db_error)?;
    if products.len() != items.len() || prices.len() != items.len() {
        return Err(StatusCode::CONFLICT);
    }

    let mut total = 0_i64;
    let mut lines = Vec::with_capacity(items.len());
    for (((product_id, quantity), (_, sku, title)), (_, unit_amount_minor, rule_version)) in
        items.into_iter().zip(products).zip(prices)
    {
        total = unit_amount_minor
            .checked_mul(quantity.into())
            .and_then(|subtotal| total.checked_add(subtotal))
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
        lines.push(CheckoutLine {
            product_id,
            sku,
            title,
            quantity,
            unit_amount_minor,
            rule_version,
        });
    }
    Ok((lines, total))
}

pub(crate) async fn status(
    customer: Customer,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CheckoutStatus>, StatusCode> {
    let databases = state
        .checkout
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let checkout = sqlx::query_as::<_, Checkout>(
        "SELECT c.id, c.cart_id, c.cart_version, c.sandbox_approve, c.status, c.failure_reason_code
         FROM checkout c JOIN order_customer owner ON owner.id = c.customer_id
         WHERE c.id = $1 AND owner.issuer = $2 AND owner.subject = $3",
    )
    .bind(id)
    .bind(customer.issuer)
    .bind(customer.subject)
    .fetch_optional(&databases.order)
    .await
    .map_err(db_error)?
    .ok_or(StatusCode::FORBIDDEN)?;
    Ok(Json(checkout.into()))
}

pub(crate) async fn order(
    customer: Customer,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Order>, StatusCode> {
    let databases = state
        .checkout
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT c.total_minor FROM checkout c JOIN order_customer owner ON owner.id = c.customer_id
         WHERE c.id = $1 AND c.status = 'confirmed' AND owner.issuer = $2 AND owner.subject = $3",
    )
    .bind(id)
    .bind(customer.issuer)
    .bind(customer.subject)
    .fetch_optional(&databases.order)
    .await
    .map_err(db_error)?
    .ok_or(StatusCode::FORBIDDEN)?;

    let items = sqlx::query_as::<_, CheckoutLine>(
        "SELECT product_id, sku, title, quantity, unit_amount_minor, rule_version
         FROM checkout_item WHERE checkout_id = $1 ORDER BY product_id",
    )
    .bind(id)
    .fetch_all(&databases.order)
    .await
    .map_err(db_error)?
    .into_iter()
    .map(|line| OrderLine {
        product_id: line.product_id,
        sku: line.sku,
        title: line.title,
        quantity: line.quantity,
        unit_amount_minor: line.unit_amount_minor.to_string(),
        rule_version: line.rule_version,
    })
    .collect();
    Ok(Json(Order {
        id,
        status: "confirmed",
        currency: "THB",
        total_minor: total.to_string(),
        items,
    }))
}

pub(crate) async fn run(databases: CheckoutDatabases, inventory_db: PgPool) {
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut expiry = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Err(err) = process_next(&databases, &inventory_db).await {
                    error!(error = %err, "checkout worker could not process a queued checkout");
                }
            }
            _ = expiry.tick() => {
                if let Err(err) = inventory::expire(&inventory_db).await {
                    error!(error = %err, "inventory reservation expiry failed");
                }
            }
        }
    }
}

async fn process_next(
    databases: &CheckoutDatabases,
    inventory_db: &PgPool,
) -> Result<(), sqlx::Error> {
    let mut tx = databases.order.begin().await?;
    let pending = sqlx::query_as::<_, (Uuid, Uuid, i64, bool)>(
        "SELECT id, customer_id, total_minor, sandbox_approve FROM checkout
         WHERE status = 'created' AND next_attempt_at <= now()
         ORDER BY next_attempt_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, customer_id, total_minor, sandbox_approve)) = pending else {
        return Ok(());
    };
    let result = process(
        &mut tx,
        databases,
        inventory_db,
        id,
        customer_id,
        total_minor,
        sandbox_approve,
    )
    .await;
    if let Err(error) = result {
        tx.rollback().await?;
        sqlx::query(
            "UPDATE checkout SET next_attempt_at = now() + interval '5 seconds'
             WHERE id = $1 AND status = 'created'",
        )
        .bind(id)
        .execute(&databases.order)
        .await?;
        return Err(error);
    }
    tx.commit().await
}

async fn process(
    tx: &mut Transaction<'_, Postgres>,
    databases: &CheckoutDatabases,
    inventory_db: &PgPool,
    id: Uuid,
    customer_id: Uuid,
    total_minor: i64,
    sandbox_approve: bool,
) -> Result<(), sqlx::Error> {
    let items = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT product_id, quantity FROM checkout_item WHERE checkout_id = $1 ORDER BY product_id",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await?;
    if !inventory::reserve(inventory_db, id, &items).await? {
        // A retry after releasing stock for a decline must preserve that outcome.
        if payment::was_declined(&databases.payment, id).await? {
            return fail(tx, id, "payment_declined").await;
        }
        payment::void(&databases.payment, id).await?;
        return fail(tx, id, "stock_unavailable").await;
    }
    let outcome = if sandbox_approve {
        payment::SandboxOutcome::Approve
    } else {
        payment::SandboxOutcome::Decline
    };
    if !payment::authorize(&databases.payment, id, total_minor, outcome).await? {
        inventory::release(inventory_db, id).await?;
        return fail(tx, id, "payment_declined").await;
    }
    if !inventory::allocate(inventory_db, id).await? {
        payment::void(&databases.payment, id).await?;
        inventory::release(inventory_db, id).await?;
        return fail(tx, id, "reservation_expired").await;
    }

    // Retrying after a lost commit reuses the payment authorization and allocation.
    sqlx::query("UPDATE checkout SET status = 'confirmed' WHERE id = $1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO order_outbox (order_id, payload)
         VALUES ($1, jsonb_build_object('order_id', $1::uuid, 'customer_id', $2::uuid,
                                        'currency', 'THB', 'grand_total_minor', $3::text))",
    )
    .bind(id)
    .bind(customer_id)
    .bind(total_minor.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn fail(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE checkout SET status = 'failed', failure_reason_code = $2 WHERE id = $1")
        .bind(id)
        .bind(reason)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn db_error(err: sqlx::Error) -> StatusCode {
    error!(error = %err, "checkout query failed");
    StatusCode::INTERNAL_SERVER_ERROR
}
