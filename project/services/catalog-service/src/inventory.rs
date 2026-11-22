use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use diesel::{
    deserialize::{self, FromSql},
    pg::{Pg, PgValue},
    result::Error as DieselError,
    sql_query,
    sql_types::{Bool, Integer, Text, Uuid as SqlUuid},
    OptionalExtension, QueryableByName,
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde::{
    de::value::{Error as ValueError, StrDeserializer},
    Deserialize, Serialize,
};
use tracing::error;
use uuid::Uuid;

use crate::{
    auth::Customer,
    db::{connection, Error, PgPool},
    AppState,
};

const EXPIRATION_BATCH_SIZE: u64 = 100;

#[derive(QueryableByName)]
struct AvailabilityRow {
    #[diesel(sql_type = Bool)]
    available: bool,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReservationStatus {
    Reserved,
    Allocated,
    Released,
    Expired,
    Dispatched,
}

impl FromSql<Text, Pg> for ReservationStatus {
    fn from_sql(value: PgValue<'_>) -> deserialize::Result<Self> {
        let status = std::str::from_utf8(value.as_bytes())?;
        Ok(Self::deserialize(StrDeserializer::<ValueError>::new(
            status,
        ))?)
    }
}

impl ReservationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Allocated => "allocated",
            Self::Released => "released",
            Self::Expired => "expired",
            Self::Dispatched => "dispatched",
        }
    }
}

#[derive(QueryableByName)]
struct ReservationStatusRow {
    #[diesel(sql_type = Text)]
    status: ReservationStatus,
}

#[derive(QueryableByName)]
struct ReservationId {
    #[diesel(sql_type = SqlUuid)]
    checkout_id: Uuid,
}

#[derive(QueryableByName)]
struct ReservationItem {
    #[diesel(sql_type = SqlUuid)]
    product_id: Uuid,
    #[diesel(sql_type = Integer)]
    quantity: i32,
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

    let mut conn = connection(&state.inventory_db).await.map_err(stock_error)?;
    let available = sql_query(
        "SELECT on_hand - reserved - allocated >= $2 AS available FROM stock_record WHERE product_id = $1",
    )
    .bind::<SqlUuid, _>(product_id)
    .bind::<Integer, _>(query.quantity)
    .get_result::<AvailabilityRow>(&mut conn)
    .await
    .optional()
    .map_err(|err| stock_error(err.into()))?
    .ok_or(StatusCode::NOT_FOUND)?
    .available;

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

    connection(pool)
        .await?
        .transaction(async move |tx| {
            let inserted = sql_query(
                "INSERT INTO checkout_reservation (checkout_id) VALUES ($1) ON CONFLICT DO NOTHING",
            )
            .bind::<SqlUuid, _>(checkout_id)
            .execute(tx)
            .await?;

            if inserted == 0 {
                let status = lock_status(tx, checkout_id).await?;
                let matches = matches!(
                    status,
                    Some(ReservationStatus::Reserved | ReservationStatus::Allocated)
                ) && load_items(tx, checkout_id).await? == items;
                return Ok(matches
                    && (status == Some(ReservationStatus::Allocated)
                        || is_live(tx, checkout_id).await?));
            }

            for (product_id, quantity) in items {
                let held = sql_query(
                    "UPDATE stock_record SET reserved = reserved + $2
                 WHERE product_id = $1 AND on_hand - reserved - allocated >= $2",
                )
                .bind::<SqlUuid, _>(product_id)
                .bind::<Integer, _>(quantity)
                .execute(tx)
                .await?;
                if held == 0 {
                    return Err(DieselError::RollbackTransaction.into());
                }
                sql_query(
                    "INSERT INTO checkout_reservation_item (checkout_id, product_id, quantity)
                 VALUES ($1, $2, $3)",
                )
                .bind::<SqlUuid, _>(checkout_id)
                .bind::<SqlUuid, _>(product_id)
                .bind::<Integer, _>(quantity)
                .execute(tx)
                .await?;
            }

            // Start the hold window after acquiring all stock locks.
            sql_query(
                "UPDATE checkout_reservation SET expires_at = DEFAULT WHERE checkout_id = $1",
            )
            .bind::<SqlUuid, _>(checkout_id)
            .execute(tx)
            .await?;
            Ok(true)
        })
        .await
        .or_else(reservation_rollback)
}

// A declined transition may have already updated stock for earlier items.
fn reservation_rollback(err: Error) -> Result<bool, Error> {
    match err {
        Error::Query(DieselError::RollbackTransaction) => Ok(false),
        other => Err(other),
    }
}

pub(crate) async fn allocate(pool: &PgPool, checkout_id: Uuid) -> Result<bool, Error> {
    connection(pool)
        .await?
        .transaction(async |tx| {
            match lock_status(tx, checkout_id).await? {
                Some(ReservationStatus::Allocated) => return Ok(true),
                Some(ReservationStatus::Reserved) => {}
                _ => return Ok(false),
            }

            if !is_live(tx, checkout_id).await? {
                finish_reserved(tx, checkout_id, ReservationStatus::Expired).await?;
                return Ok(false);
            }

            if !finish_reserved(tx, checkout_id, ReservationStatus::Allocated).await? {
                // The reservation expired while waiting for a stock lock.
                return Err(DieselError::RollbackTransaction.into());
            }
            Ok(true)
        })
        .await
        .or_else(reservation_rollback)
}

pub(crate) async fn release(pool: &PgPool, checkout_id: Uuid) -> Result<(), Error> {
    connection(pool)
        .await?
        .transaction(async |tx| {
            if lock_status(tx, checkout_id).await? == Some(ReservationStatus::Reserved) {
                finish_reserved(tx, checkout_id, ReservationStatus::Released).await?;
            }
            Ok(())
        })
        .await
}

pub(crate) async fn expire(pool: &PgPool) -> Result<u64, Error> {
    let mut expired = 0;
    while expired < EXPIRATION_BATCH_SIZE {
        // Use a transaction per reservation so every transition locks stock in product order.
        let changed = connection(pool)
            .await?
            .transaction::<_, Error, _>(async |tx| {
                let reservation = sql_query(
                    "SELECT checkout_id FROM checkout_reservation
                 WHERE status = 'reserved' AND expires_at <= statement_timestamp()
                 ORDER BY expires_at, checkout_id LIMIT 1 FOR UPDATE SKIP LOCKED",
                )
                .get_result::<ReservationId>(tx)
                .await
                .optional()?;
                let Some(reservation) = reservation else {
                    return Ok(false);
                };
                finish_reserved(tx, reservation.checkout_id, ReservationStatus::Expired).await?;
                Ok(true)
            })
            .await?;
        if !changed {
            break;
        }
        expired += 1;
    }
    Ok(expired)
}

async fn lock_status(
    tx: &mut AsyncPgConnection,
    checkout_id: Uuid,
) -> Result<Option<ReservationStatus>, Error> {
    Ok(sql_query(
        "SELECT status::text AS status FROM checkout_reservation WHERE checkout_id = $1 FOR UPDATE",
    )
    .bind::<SqlUuid, _>(checkout_id)
    .get_result::<ReservationStatusRow>(tx)
    .await
    .optional()?
    .map(|row| row.status))
}

async fn is_live(tx: &mut AsyncPgConnection, checkout_id: Uuid) -> Result<bool, Error> {
    Ok(sql_query(
        "SELECT expires_at > clock_timestamp() AS available
         FROM checkout_reservation WHERE checkout_id = $1",
    )
    .bind::<SqlUuid, _>(checkout_id)
    .get_result::<AvailabilityRow>(tx)
    .await?
    .available)
}

async fn load_items(
    tx: &mut AsyncPgConnection,
    checkout_id: Uuid,
) -> Result<Vec<(Uuid, i32)>, Error> {
    let items = sql_query(
        "SELECT product_id, quantity FROM checkout_reservation_item
         WHERE checkout_id = $1 ORDER BY product_id",
    )
    .bind::<SqlUuid, _>(checkout_id)
    .load::<ReservationItem>(tx)
    .await?;
    Ok(items
        .into_iter()
        .map(|row| (row.product_id, row.quantity))
        .collect())
}

async fn finish_reserved(
    tx: &mut AsyncPgConnection,
    checkout_id: Uuid,
    next: ReservationStatus,
) -> Result<bool, Error> {
    for (product_id, quantity) in load_items(tx, checkout_id).await? {
        sql_query(
            "UPDATE stock_record SET reserved = reserved - $2,
             allocated = allocated + CASE WHEN $3 THEN $2 ELSE 0 END
             WHERE product_id = $1",
        )
        .bind::<SqlUuid, _>(product_id)
        .bind::<Integer, _>(quantity)
        .bind::<Bool, _>(next == ReservationStatus::Allocated)
        .execute(tx)
        .await?;
    }

    let changed = sql_query(
        "UPDATE checkout_reservation SET status = $2::checkout_reservation_status
         WHERE checkout_id = $1 AND status = 'reserved'
           AND ($2 <> 'allocated' OR expires_at > clock_timestamp())",
    )
    .bind::<SqlUuid, _>(checkout_id)
    .bind::<Text, _>(next.as_str())
    .execute(tx)
    .await?;
    Ok(changed == 1)
}

pub(crate) async fn dispatch(pool: &PgPool, order_id: Uuid) -> Result<bool, Error> {
    connection(pool)
        .await?
        .transaction(async |tx| {
            match lock_status(tx, order_id).await? {
                Some(ReservationStatus::Dispatched) => return Ok(true),
                Some(ReservationStatus::Allocated) => {}
                _ => return Ok(false),
            }
            for (product_id, quantity) in load_items(tx, order_id).await? {
                sql_query(
                    "UPDATE stock_record SET on_hand = on_hand - $2, allocated = allocated - $2
                 WHERE product_id = $1",
                )
                .bind::<SqlUuid, _>(product_id)
                .bind::<Integer, _>(quantity)
                .execute(tx)
                .await?;
            }
            sql_query(
                "UPDATE checkout_reservation SET status = 'dispatched' WHERE checkout_id = $1",
            )
            .bind::<SqlUuid, _>(order_id)
            .execute(tx)
            .await?;
            Ok(true)
        })
        .await
}

#[derive(Serialize, QueryableByName)]
pub(crate) struct Stock {
    #[diesel(sql_type = SqlUuid)]
    product_id: Uuid,
    #[diesel(sql_type = Integer)]
    on_hand: i32,
    #[diesel(sql_type = Integer)]
    reserved: i32,
    #[diesel(sql_type = Integer)]
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
    let mut conn = connection(&state.inventory_db).await.map_err(stock_error)?;
    sql_query(
        "SELECT product_id, on_hand, reserved, allocated FROM stock_record WHERE product_id = $1",
    )
    .bind::<SqlUuid, _>(product_id)
    .get_result::<Stock>(&mut conn)
    .await
    .optional()
    .map_err(|err| stock_error(err.into()))?
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
    let mut conn = connection(&state.inventory_db).await.map_err(stock_error)?;
    conn.transaction::<_, Error, _>(async move |tx| {
        let stock = sql_query(
            "SELECT product_id, on_hand, reserved, allocated
             FROM stock_record WHERE product_id = $1 FOR UPDATE",
        )
        .bind::<SqlUuid, _>(product_id)
        .get_result::<Stock>(tx)
        .await
        .optional()?
        .ok_or(StatusCode::NOT_FOUND)?;

        if stock.on_hand != change.expected_on_hand
            || change.on_hand < stock.reserved + stock.allocated
        {
            return Err(StatusCode::CONFLICT.into());
        }
        if change.on_hand == stock.on_hand {
            return Ok(Json(stock));
        }

        sql_query("UPDATE stock_record SET on_hand = $2 WHERE product_id = $1")
            .bind::<SqlUuid, _>(product_id)
            .bind::<Integer, _>(change.on_hand)
            .execute(tx)
            .await?;
        sql_query(
            "INSERT INTO stock_adjustment
                 (product_id, before_on_hand, after_on_hand, actor_issuer, actor_subject)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind::<SqlUuid, _>(product_id)
        .bind::<Integer, _>(stock.on_hand)
        .bind::<Integer, _>(change.on_hand)
        .bind::<Text, _>(customer.issuer)
        .bind::<Text, _>(customer.subject)
        .execute(tx)
        .await?;
        Ok(Json(Stock {
            on_hand: change.on_hand,
            ..stock
        }))
    })
    .await
    .map_err(|err| err.into_status(stock_error))
}

fn stock_error(err: Error) -> StatusCode {
    error!(error = %err, "inventory operation failed");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{with_test_database, DbMigrations};

    const MUG: Uuid = Uuid::from_u128(0x019b0000_0000_7000_8000_000000000002);
    const SECOND_PRODUCT: Uuid = Uuid::from_u128(0x019b0000_0000_7000_8000_000000000003);

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn holds_are_retry_safe_and_allocated_stock_stays_unavailable() {
        with_test_database(DbMigrations::Inventory, async |pool| {
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
            assert_eq!(stock_counts(&pool, MUG).await, (15, 15, 0));
            assert!(!reserve(&pool, checkout, &[(MUG, 10)]).await.unwrap());
            assert!(allocate(&pool, checkout).await.unwrap());
            assert!(allocate(&pool, checkout).await.unwrap());
            assert!(reserve(&pool, checkout, &[(MUG, 12)]).await.unwrap());
            release(&pool, checkout).await.unwrap();
            assert!(!reserve(&pool, Uuid::from_u128(2), &[(MUG, 1)])
                .await
                .unwrap());
            assert_eq!(stock_counts(&pool, MUG).await, (15, 3, 12));

            // A worker can retry after debiting inventory but before advancing the shipment.
            let (first, second) =
                tokio::join!(dispatch(&pool, checkout), dispatch(&pool, checkout));
            assert!(first.unwrap());
            assert!(second.unwrap());
            assert_eq!(stock_counts(&pool, MUG).await, (3, 3, 0));
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn competing_multi_item_holds_roll_back_and_expiry_reopens_stock() {
        with_test_database(DbMigrations::Inventory, async |pool| {
            let first = Uuid::from_u128(1);
            let second = Uuid::from_u128(2);
            let ordered = [(MUG, 12), (SECOND_PRODUCT, 1)];
            let reversed = [(SECOND_PRODUCT, 1), (MUG, 12)];
            assert!(!reserve(&pool, first, &ordered).await.unwrap());
            assert_eq!(stock_counts(&pool, MUG).await, (15, 3, 0));
            sql_query("INSERT INTO stock_record (product_id, on_hand, reserved) VALUES ($1, 1, 0)")
                .bind::<SqlUuid, _>(SECOND_PRODUCT)
                .execute(&mut connection(&pool).await.unwrap())
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
            sql_query(
                "UPDATE checkout_reservation SET expires_at = clock_timestamp()
                 WHERE checkout_id = $1",
            )
            .bind::<SqlUuid, _>(winner)
            .execute(&mut connection(&pool).await.unwrap())
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
            assert_eq!(stock_counts(&pool, MUG).await, (15, 3, 0));
            assert_eq!(stock_counts(&pool, SECOND_PRODUCT).await, (1, 0, 0));
        })
        .await;
    }

    async fn stock_counts(pool: &PgPool, product_id: Uuid) -> (i32, i32, i32) {
        let stock = sql_query(
            "SELECT product_id, on_hand, reserved, allocated
             FROM stock_record WHERE product_id = $1",
        )
        .bind::<SqlUuid, _>(product_id)
        .get_result::<Stock>(&mut connection(pool).await.unwrap())
        .await
        .unwrap();
        (stock.on_hand, stock.reserved, stock.allocated)
    }
}
