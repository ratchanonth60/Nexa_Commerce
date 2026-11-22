use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Uuid, Error, FromRow, PgPool, Postgres, Transaction};
use tracing::error;

use crate::{auth::Customer, AppState};

const EXPIRATION_BATCH_SIZE: u64 = 100;

#[derive(Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "checkout_reservation_status", rename_all = "snake_case")]
enum ReservationStatus {
    Reserved,
    Allocated,
    Released,
    Expired,
    Dispatched,
}

#[derive(Deserialize)]
pub(crate) struct AvailabilityQuery {
    product_id: String,
    quantity: i32,
}

#[derive(Serialize)]
pub(crate) struct Availability {
    product_id: String,
    quantity: i32,
    available: bool,
}

pub(crate) async fn get_availability(
    State(state): State<AppState>,
    Query(query): Query<AvailabilityQuery>,
) -> Result<Json<Availability>, StatusCode> {
    let product_id = Uuid::parse_str(&query.product_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    if !(1..=9999).contains(&query.quantity) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let available = sqlx::query_scalar::<_, bool>(
        "SELECT on_hand - reserved - allocated >= $2 FROM stock_record WHERE product_id = $1",
    )
    .bind(product_id)
    .bind(query.quantity)
    .fetch_optional(&state.inventory_db)
    .await
    .map_err(|err| {
        error!(error = %err, "inventory query failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(Availability {
        product_id: product_id.to_string(),
        quantity: query.quantity,
        available,
    }))
}

pub(crate) async fn reserve(
    pool: &PgPool,
    checkout_id: Uuid,
    items: &[(Uuid, i32)],
) -> Result<bool, Error> {
    let mut items = items.to_vec();
    items.sort_unstable_by_key(|&(product_id, _)| product_id);
    if items.is_empty()
        || items
            .iter()
            .any(|&(_, quantity)| !(1..=9999).contains(&quantity))
        || items.windows(2).any(|pair| pair[0].0 == pair[1].0)
    {
        return Err(Error::InvalidArgument(
            "reservations require unique product IDs and quantities between 1 and 9999".into(),
        ));
    }

    let mut tx = pool.begin().await?;
    let created = sqlx::query(
        "INSERT INTO checkout_reservation (checkout_id) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(checkout_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;

    if !created {
        let status = lock_status(&mut tx, checkout_id).await?;
        let matches = matches!(
            status,
            Some(ReservationStatus::Reserved | ReservationStatus::Allocated)
        ) && load_items(&mut tx, checkout_id).await? == items;
        let active = matches
            && (status == Some(ReservationStatus::Allocated)
                || is_live(&mut tx, checkout_id).await?);
        tx.commit().await?;
        return Ok(active);
    }

    for (product_id, quantity) in items {
        let held = sqlx::query(
            "UPDATE stock_record SET reserved = reserved + $2
             WHERE product_id = $1 AND on_hand - reserved - allocated >= $2",
        )
        .bind(product_id)
        .bind(quantity)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !held {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO checkout_reservation_item (checkout_id, product_id, quantity)
             VALUES ($1, $2, $3)",
        )
        .bind(checkout_id)
        .bind(product_id)
        .bind(quantity)
        .execute(&mut *tx)
        .await?;
    }

    // Start the hold window after acquiring all stock locks.
    sqlx::query("UPDATE checkout_reservation SET expires_at = DEFAULT WHERE checkout_id = $1")
        .bind(checkout_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

pub(crate) async fn allocate(pool: &PgPool, checkout_id: Uuid) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    match lock_status(&mut tx, checkout_id).await? {
        Some(ReservationStatus::Allocated) => {
            tx.commit().await?;
            return Ok(true);
        }
        Some(ReservationStatus::Reserved) => {}
        _ => {
            tx.commit().await?;
            return Ok(false);
        }
    }

    if !is_live(&mut tx, checkout_id).await? {
        finish_reserved(&mut tx, checkout_id, ReservationStatus::Expired).await?;
        tx.commit().await?;
        return Ok(false);
    }

    if !finish_reserved(&mut tx, checkout_id, ReservationStatus::Allocated).await? {
        // The reservation expired while waiting for a stock lock.
        tx.rollback().await?;
        return Ok(false);
    }
    tx.commit().await?;
    Ok(true)
}

pub(crate) async fn release(pool: &PgPool, checkout_id: Uuid) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    if lock_status(&mut tx, checkout_id).await? == Some(ReservationStatus::Reserved) {
        finish_reserved(&mut tx, checkout_id, ReservationStatus::Released).await?;
    }
    tx.commit().await
}

pub(crate) async fn expire(pool: &PgPool) -> Result<u64, Error> {
    let mut expired = 0;
    while expired < EXPIRATION_BATCH_SIZE {
        // Use a transaction per reservation so every transition locks stock in product order.
        let mut tx = pool.begin().await?;
        let checkout_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT checkout_id FROM checkout_reservation
             WHERE status = 'reserved' AND expires_at <= statement_timestamp()
             ORDER BY expires_at, checkout_id LIMIT 1 FOR UPDATE SKIP LOCKED",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(checkout_id) = checkout_id else {
            tx.commit().await?;
            break;
        };
        finish_reserved(&mut tx, checkout_id, ReservationStatus::Expired).await?;
        tx.commit().await?;
        expired += 1;
    }
    Ok(expired)
}

async fn lock_status(
    tx: &mut Transaction<'_, Postgres>,
    checkout_id: Uuid,
) -> Result<Option<ReservationStatus>, Error> {
    sqlx::query_scalar("SELECT status FROM checkout_reservation WHERE checkout_id = $1 FOR UPDATE")
        .bind(checkout_id)
        .fetch_optional(&mut **tx)
        .await
}

async fn is_live(tx: &mut Transaction<'_, Postgres>, checkout_id: Uuid) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT expires_at > clock_timestamp() FROM checkout_reservation WHERE checkout_id = $1",
    )
    .bind(checkout_id)
    .fetch_one(&mut **tx)
    .await
}

async fn load_items(
    tx: &mut Transaction<'_, Postgres>,
    checkout_id: Uuid,
) -> Result<Vec<(Uuid, i32)>, Error> {
    sqlx::query_as(
        "SELECT product_id, quantity FROM checkout_reservation_item
         WHERE checkout_id = $1 ORDER BY product_id",
    )
    .bind(checkout_id)
    .fetch_all(&mut **tx)
    .await
}

async fn finish_reserved(
    tx: &mut Transaction<'_, Postgres>,
    checkout_id: Uuid,
    next: ReservationStatus,
) -> Result<bool, Error> {
    for (product_id, quantity) in load_items(tx, checkout_id).await? {
        sqlx::query(
            "UPDATE stock_record SET reserved = reserved - $2,
             allocated = allocated + CASE WHEN $3 THEN $2 ELSE 0 END
             WHERE product_id = $1",
        )
        .bind(product_id)
        .bind(quantity)
        .bind(next == ReservationStatus::Allocated)
        .execute(&mut **tx)
        .await?;
    }

    let changed = sqlx::query(
        "UPDATE checkout_reservation SET status = $2
         WHERE checkout_id = $1 AND status = 'reserved'
           AND ($2 <> 'allocated' OR expires_at > clock_timestamp())",
    )
    .bind(checkout_id)
    .bind(next)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1;
    Ok(changed)
}

pub(crate) async fn dispatch(pool: &PgPool, order_id: Uuid) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    match lock_status(&mut tx, order_id).await? {
        Some(ReservationStatus::Dispatched) => return Ok(true),
        Some(ReservationStatus::Allocated) => {}
        _ => return Ok(false),
    }
    for (product_id, quantity) in load_items(&mut tx, order_id).await? {
        sqlx::query(
            "UPDATE stock_record SET on_hand = on_hand - $2, allocated = allocated - $2
             WHERE product_id = $1",
        )
        .bind(product_id)
        .bind(quantity)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE checkout_reservation SET status = 'dispatched' WHERE checkout_id = $1")
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(Serialize, FromRow)]
pub(crate) struct Stock {
    product_id: Uuid,
    on_hand: i32,
    reserved: i32,
    allocated: i32,
}

#[derive(Deserialize)]
pub(crate) struct StockAdjustment {
    on_hand: i32,
    expected_on_hand: i32,
}

pub(crate) async fn read_stock(
    customer: Customer,
    State(state): State<AppState>,
    Path(product_id): Path<Uuid>,
) -> Result<Json<Stock>, StatusCode> {
    if !customer.operator {
        return Err(StatusCode::FORBIDDEN);
    }
    sqlx::query_as(
        "SELECT product_id, on_hand, reserved, allocated FROM stock_record WHERE product_id = $1",
    )
    .bind(product_id)
    .fetch_optional(&state.inventory_db)
    .await
    .map_err(stock_error)?
    .map(Json)
    .ok_or(StatusCode::NOT_FOUND)
}

pub(crate) async fn adjust_stock(
    customer: Customer,
    State(state): State<AppState>,
    Path(product_id): Path<Uuid>,
    Json(change): Json<StockAdjustment>,
) -> Result<Json<Stock>, StatusCode> {
    if !customer.operator {
        return Err(StatusCode::FORBIDDEN);
    }
    if change.on_hand < 0 || change.expected_on_hand < 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut tx = state.inventory_db.begin().await.map_err(stock_error)?;
    let stock = sqlx::query_as::<_, Stock>(
        "SELECT product_id, on_hand, reserved, allocated FROM stock_record WHERE product_id = $1 FOR UPDATE",
    )
    .bind(product_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(stock_error)?
    .ok_or(StatusCode::NOT_FOUND)?;

    if stock.on_hand != change.expected_on_hand || change.on_hand < stock.reserved + stock.allocated
    {
        return Err(StatusCode::CONFLICT);
    }
    if change.on_hand == stock.on_hand {
        return Ok(Json(stock));
    }

    sqlx::query("UPDATE stock_record SET on_hand = $2 WHERE product_id = $1")
        .bind(product_id)
        .bind(change.on_hand)
        .execute(&mut *tx)
        .await
        .map_err(stock_error)?;
    sqlx::query(
        "INSERT INTO stock_adjustment (product_id, before_on_hand, after_on_hand, actor_issuer, actor_subject)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(product_id)
    .bind(stock.on_hand)
    .bind(change.on_hand)
    .bind(customer.issuer)
    .bind(customer.subject)
    .execute(&mut *tx)
    .await
    .map_err(stock_error)?;
    tx.commit().await.map_err(stock_error)?;
    Ok(Json(Stock {
        on_hand: change.on_hand,
        ..stock
    }))
}

fn stock_error(err: Error) -> StatusCode {
    error!(error = %err, "inventory operation failed");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[cfg(test)]
mod tests {
    use super::*;

    const MUG: Uuid = Uuid::from_u128(0x019b0000_0000_7000_8000_000000000002);
    const SECOND_PRODUCT: Uuid = Uuid::from_u128(0x019b0000_0000_7000_8000_000000000003);

    #[sqlx::test(migrations = "./inventory-migrations")]
    #[ignore = "requires a PostgreSQL test server"]
    async fn holds_are_retry_safe_and_allocated_stock_stays_unavailable(pool: PgPool) {
        let checkout = Uuid::from_u128(1);
        assert!(matches!(
            reserve(&pool, checkout, &[(MUG, 1), (MUG, 1)]).await,
            Err(Error::InvalidArgument(_))
        ));
        assert!(!reserve(&pool, checkout, &[(MUG, 13)]).await.unwrap());
        let items = [(MUG, 12)];
        let (first, second) = tokio::join!(
            reserve(&pool, checkout, &items),
            reserve(&pool, checkout, &items)
        );
        assert!(first.unwrap());
        assert!(second.unwrap());
        assert_eq!(stock_counts(&pool, MUG).await, (15, 0));
        assert!(!reserve(&pool, checkout, &[(MUG, 10)]).await.unwrap());
        assert!(allocate(&pool, checkout).await.unwrap());
        assert!(allocate(&pool, checkout).await.unwrap());
        assert!(reserve(&pool, checkout, &[(MUG, 12)]).await.unwrap());
        release(&pool, checkout).await.unwrap();
        assert!(!reserve(&pool, Uuid::from_u128(2), &[(MUG, 1)])
            .await
            .unwrap());
        assert_eq!(stock_counts(&pool, MUG).await, (3, 12));
    }

    #[sqlx::test(migrations = "./inventory-migrations")]
    #[ignore = "requires a PostgreSQL test server"]
    async fn competing_multi_item_holds_roll_back_and_expiry_reopens_stock(pool: PgPool) {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let ordered = [(MUG, 12), (SECOND_PRODUCT, 1)];
        let reversed = [(SECOND_PRODUCT, 1), (MUG, 12)];
        assert!(!reserve(&pool, first, &ordered).await.unwrap());
        assert_eq!(stock_counts(&pool, MUG).await, (3, 0));
        sqlx::query("INSERT INTO stock_record (product_id, on_hand, reserved) VALUES ($1, 1, 0)")
            .bind(SECOND_PRODUCT)
            .execute(&pool)
            .await
            .unwrap();

        let (first_result, second_result) = tokio::join!(
            reserve(&pool, first, &ordered),
            reserve(&pool, second, &reversed)
        );
        let first_won = first_result.unwrap();
        assert_ne!(first_won, second_result.unwrap());
        let (winner, other) = if first_won {
            (first, second)
        } else {
            (second, first)
        };
        assert!(reserve(&pool, winner, &reversed).await.unwrap());
        assert_eq!(expire(&pool).await.unwrap(), 0);
        sqlx::query(
            "UPDATE checkout_reservation SET expires_at = clock_timestamp() WHERE checkout_id = $1",
        )
        .bind(winner)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!reserve(&pool, winner, &ordered).await.unwrap());
        assert_eq!(expire(&pool).await.unwrap(), 1);
        assert_eq!(expire(&pool).await.unwrap(), 0);
        assert!(!allocate(&pool, winner).await.unwrap());
        assert!(reserve(&pool, other, &ordered).await.unwrap());
        release(&pool, other).await.unwrap();
        release(&pool, other).await.unwrap();
        assert!(!allocate(&pool, other).await.unwrap());
        assert!(!reserve(&pool, other, &ordered).await.unwrap());
        assert_eq!(stock_counts(&pool, MUG).await, (3, 0));
        assert_eq!(stock_counts(&pool, SECOND_PRODUCT).await, (0, 0));
    }

    async fn stock_counts(pool: &PgPool, product_id: Uuid) -> (i32, i32) {
        sqlx::query_as("SELECT reserved, allocated FROM stock_record WHERE product_id = $1")
            .bind(product_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }
}
