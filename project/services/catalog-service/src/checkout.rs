use std::{env, io, time::Duration};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use diesel::{
    sql_query,
    sql_types::{Array, BigInt, Bool, Integer, Nullable, Text, Uuid as SqlUuid},
    OptionalExtension, QueryableByName,
};
use diesel_async::{
    pooled_connection::PoolableConnection, AsyncConnection, AsyncPgConnection, RunQueryDsl,
};
use serde::{Deserialize, Serialize};
use tracing::error;
use uuid::Uuid;

use crate::{
    auth::Customer,
    cart,
    db::{connect, connection, DbMigrations, Error, PgPool},
    inventory, payment, AppState,
};

#[derive(Clone)]
pub(crate) struct CheckoutDatabases {
    order: PgPool,
    payment: PgPool,
}

impl CheckoutDatabases {
    pub(crate) async fn ping(&self) -> Result<(), Error> {
        tokio::try_join!(crate::ping(&self.order), crate::ping(&self.payment))?;
        Ok(())
    }

    pub(crate) fn order_pool(&self) -> &PgPool {
        &self.order
    }

    pub(crate) async fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error + Send + Sync>>
    {
        let order_url = env::var("ORDER_DATABASE_URL").ok();
        let payment_url = env::var("PAYMENT_DATABASE_URL").ok();
        let sandbox = env::var("CHECKOUT_SANDBOX").ok();
        match (sandbox.as_deref(), order_url, payment_url) {
            (None | Some("false"), None, None) => Ok(None),
            (Some("true"), Some(order), Some(payment)) => {
                let order = connect(&order, DbMigrations::Order).await?;
                let payment = connect(&payment, DbMigrations::Payment).await?;
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

#[derive(QueryableByName)]
struct CustomerId {
    #[diesel(sql_type = SqlUuid)]
    id: Uuid,
}

#[derive(QueryableByName)]
struct ProductSnapshot {
    #[diesel(sql_type = Text)]
    sku: String,
    #[diesel(sql_type = Text)]
    title: String,
}

#[derive(QueryableByName)]
struct PriceSnapshot {
    #[diesel(sql_type = BigInt)]
    unit_amount_minor: i64,
    #[diesel(sql_type = Integer)]
    rule_version: i32,
}

#[derive(QueryableByName)]
struct OrderTotal {
    #[diesel(sql_type = BigInt)]
    total_minor: i64,
}

#[derive(QueryableByName)]
struct PendingCheckout {
    #[diesel(sql_type = SqlUuid)]
    id: Uuid,
    #[diesel(sql_type = SqlUuid)]
    customer_id: Uuid,
    #[diesel(sql_type = BigInt)]
    total_minor: i64,
    #[diesel(sql_type = Bool)]
    sandbox_approve: bool,
}

#[derive(QueryableByName)]
struct ItemQuantity {
    #[diesel(sql_type = SqlUuid)]
    product_id: Uuid,
    #[diesel(sql_type = Integer)]
    quantity: i32,
}

#[derive(Deserialize)]
pub(crate) struct CheckoutRequest {
    cart_id: Uuid,
    cart_version: i64,
    payment_method_token: String,
    shipping_address_id: Option<Uuid>,
}

#[derive(QueryableByName)]
struct Checkout {
    #[diesel(sql_type = SqlUuid)]
    id: Uuid,
    #[diesel(sql_type = SqlUuid)]
    cart_id: Uuid,
    #[diesel(sql_type = BigInt)]
    cart_version: i64,
    #[diesel(sql_type = Bool)]
    sandbox_approve: bool,
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = Nullable<Text>)]
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

#[derive(QueryableByName)]
struct CheckoutLine {
    #[diesel(sql_type = SqlUuid)]
    product_id: Uuid,
    #[diesel(sql_type = Text)]
    sku: String,
    #[diesel(sql_type = Text)]
    title: String,
    #[diesel(sql_type = Integer)]
    quantity: i32,
    #[diesel(sql_type = BigInt)]
    unit_amount_minor: i64,
    #[diesel(sql_type = Integer)]
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
    let customer_id = {
        let mut db = connection(&databases.order).await.map_err(db_error)?;
        let customer_id = sql_query(
            "INSERT INTO order_customer (issuer, subject) VALUES ($1, $2)
             ON CONFLICT (issuer, subject) DO UPDATE SET issuer = EXCLUDED.issuer RETURNING id",
        )
        .bind::<Text, _>(&customer.issuer)
        .bind::<Text, _>(&customer.subject)
        .get_result::<CustomerId>(&mut db)
        .await
        .map_err(db_error)?
        .id;

        if let Some(existing) = by_key(&mut db, customer_id, key).await.map_err(db_error)? {
            return replay(existing, &request, sandbox_approve);
        }
        customer_id
    };

    let (lines, total) = snapshot(&state, &customer, &request).await?;
    let mut order_db = connection(&databases.order).await.map_err(db_error)?;
    let write = order_db
        .transaction::<_, Error, _>(async |tx| {
            let checkout = sql_query(
                "INSERT INTO checkout
                    (customer_id, idempotency_key, cart_id, cart_version, sandbox_approve, total_minor)
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING
                 RETURNING id, cart_id, cart_version, sandbox_approve, status, failure_reason_code",
            )
            .bind::<SqlUuid, _>(customer_id)
            .bind::<Text, _>(key)
            .bind::<SqlUuid, _>(request.cart_id)
            .bind::<BigInt, _>(request.cart_version)
            .bind::<Bool, _>(sandbox_approve)
            .bind::<BigInt, _>(total)
            .get_result::<Checkout>(tx)
            .await
            .optional()?
            .ok_or(StatusCode::CONFLICT)?;

            for line in lines {
                sql_query(
                    "INSERT INTO checkout_item
                        (checkout_id, product_id, sku, title, quantity, unit_amount_minor, rule_version)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)",
                )
                .bind::<SqlUuid, _>(checkout.id)
                .bind::<SqlUuid, _>(line.product_id)
                .bind::<Text, _>(line.sku)
                .bind::<Text, _>(line.title)
                .bind::<Integer, _>(line.quantity)
                .bind::<BigInt, _>(line.unit_amount_minor)
                .bind::<Integer, _>(line.rule_version)
                .execute(tx)
                .await?;
            }
            Ok(checkout)
        })
        .await;
    match write {
        Ok(checkout) => Ok((StatusCode::ACCEPTED, Json(checkout.into()))),
        Err(Error::Status(StatusCode::CONFLICT)) => {
            let existing = by_key(&mut order_db, customer_id, key)
                .await
                .map_err(db_error)?
                .ok_or(StatusCode::CONFLICT)?;
            replay(existing, &request, sandbox_approve)
        }
        Err(err) => Err(db_error(err)),
    }
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
    db: &mut AsyncPgConnection,
    customer_id: Uuid,
    key: &str,
) -> Result<Option<Checkout>, Error> {
    sql_query(
        "SELECT id, cart_id, cart_version, sandbox_approve, status, failure_reason_code
         FROM checkout WHERE customer_id = $1 AND idempotency_key = $2",
    )
    .bind::<SqlUuid, _>(customer_id)
    .bind::<Text, _>(key)
    .get_result(db)
    .await
    .optional()
    .map_err(Error::from)
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
    let (version, items) = cart::checkout_snapshot(
        &mut *connection(&state.cart_db).await.map_err(db_error)?,
        request.cart_id,
        customer,
    )
    .await?;
    if version != request.cart_version || items.is_empty() {
        return Err(StatusCode::CONFLICT);
    }
    let ids = items.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let products = sql_query(
        "SELECT sku, title FROM product WHERE status = 'active' AND id = ANY($1) ORDER BY id",
    )
    .bind::<Array<SqlUuid>, _>(&ids)
    .load::<ProductSnapshot>(&mut *connection(&state.db).await.map_err(db_error)?)
    .await
    .map_err(db_error)?;
    let prices = sql_query(
        "SELECT unit_amount_minor, rule_version FROM price
         WHERE product_id = ANY($1) AND currency = 'THB' AND channel = 'storefront'
         ORDER BY product_id",
    )
    .bind::<Array<SqlUuid>, _>(&ids)
    .load::<PriceSnapshot>(&mut *connection(&state.pricing_db).await.map_err(db_error)?)
    .await
    .map_err(db_error)?;
    if products.len() != items.len() || prices.len() != items.len() {
        return Err(StatusCode::CONFLICT);
    }

    let mut total = 0_i64;
    let mut lines = Vec::with_capacity(items.len());
    for (((product_id, quantity), product), price) in items.into_iter().zip(products).zip(prices) {
        total = price
            .unit_amount_minor
            .checked_mul(quantity.into())
            .and_then(|subtotal| total.checked_add(subtotal))
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
        lines.push(CheckoutLine {
            product_id,
            sku: product.sku,
            title: product.title,
            quantity,
            unit_amount_minor: price.unit_amount_minor,
            rule_version: price.rule_version,
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
    let checkout = sql_query(
        "SELECT c.id, c.cart_id, c.cart_version, c.sandbox_approve, c.status, c.failure_reason_code
         FROM checkout c JOIN order_customer owner ON owner.id = c.customer_id
         WHERE c.id = $1 AND owner.issuer = $2 AND owner.subject = $3",
    )
    .bind::<SqlUuid, _>(id)
    .bind::<Text, _>(customer.issuer)
    .bind::<Text, _>(customer.subject)
    .get_result::<Checkout>(&mut *connection(&databases.order).await.map_err(db_error)?)
    .await
    .optional()
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
    let mut order_db = connection(&databases.order).await.map_err(db_error)?;
    let total = sql_query(
        "SELECT c.total_minor FROM checkout c JOIN order_customer owner ON owner.id = c.customer_id
         WHERE c.id = $1 AND c.status = 'confirmed' AND owner.issuer = $2 AND owner.subject = $3",
    )
    .bind::<SqlUuid, _>(id)
    .bind::<Text, _>(customer.issuer)
    .bind::<Text, _>(customer.subject)
    .get_result::<OrderTotal>(&mut order_db)
    .await
    .optional()
    .map_err(db_error)?
    .ok_or(StatusCode::FORBIDDEN)?;

    let items = sql_query(
        "SELECT product_id, sku, title, quantity, unit_amount_minor, rule_version
         FROM checkout_item WHERE checkout_id = $1 ORDER BY product_id",
    )
    .bind::<SqlUuid, _>(id)
    .load::<CheckoutLine>(&mut order_db)
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
        total_minor: total.total_minor.to_string(),
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

async fn process_next(databases: &CheckoutDatabases, inventory_db: &PgPool) -> Result<(), Error> {
    let mut db = connection(&databases.order).await?;
    let mut retry = None;
    let result = db
        .transaction::<_, Error, _>(async |tx| {
            let pending = sql_query(
                "SELECT id, customer_id, total_minor, sandbox_approve FROM checkout
                 WHERE status = 'created' AND next_attempt_at <= now()
                 ORDER BY next_attempt_at, id LIMIT 1 FOR UPDATE SKIP LOCKED",
            )
            .get_result::<PendingCheckout>(tx)
            .await
            .optional()?;
            if let Some(pending) = pending {
                let id = pending.id;
                if let Err(error) = process(tx, databases, inventory_db, pending).await {
                    retry = Some(id);
                    return Err(error);
                }
            }
            Ok(())
        })
        .await;

    if let Some(id) = retry {
        // Schedule the retry only after the worker's transaction rolled back.
        if db.is_broken() {
            return result;
        }
        sql_query(
            "UPDATE checkout SET next_attempt_at = now() + interval '5 seconds'
             WHERE id = $1 AND status = 'created'",
        )
        .bind::<SqlUuid, _>(id)
        .execute(&mut db)
        .await?;
    }
    result
}

async fn process(
    tx: &mut AsyncPgConnection,
    databases: &CheckoutDatabases,
    inventory_db: &PgPool,
    pending: PendingCheckout,
) -> Result<(), Error> {
    let PendingCheckout {
        id,
        customer_id,
        total_minor,
        sandbox_approve,
    } = pending;
    let items: Vec<_> = sql_query(
        "SELECT product_id, quantity FROM checkout_item WHERE checkout_id = $1 ORDER BY product_id",
    )
    .bind::<SqlUuid, _>(id)
    .load::<ItemQuantity>(tx)
    .await?
    .into_iter()
    .map(|item| (item.product_id, item.quantity))
    .collect();
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
    sql_query("UPDATE checkout SET status = 'confirmed' WHERE id = $1")
        .bind::<SqlUuid, _>(id)
        .execute(tx)
        .await?;
    sql_query(
        "INSERT INTO order_outbox (order_id, payload)
         VALUES ($1, jsonb_build_object('order_id', $1::uuid, 'customer_id', $2::uuid,
                                        'currency', 'THB', 'grand_total_minor', $3::text))",
    )
    .bind::<SqlUuid, _>(id)
    .bind::<SqlUuid, _>(customer_id)
    .bind::<Text, _>(total_minor.to_string())
    .execute(tx)
    .await?;
    Ok(())
}

async fn fail(tx: &mut AsyncPgConnection, id: Uuid, reason: &str) -> Result<(), Error> {
    sql_query("UPDATE checkout SET status = 'failed', failure_reason_code = $2 WHERE id = $1")
        .bind::<SqlUuid, _>(id)
        .bind::<Text, _>(reason)
        .execute(tx)
        .await?;
    Ok(())
}

fn db_error(err: impl std::fmt::Display) -> StatusCode {
    error!(error = %err, "checkout query failed");
    StatusCode::INTERNAL_SERVER_ERROR
}
