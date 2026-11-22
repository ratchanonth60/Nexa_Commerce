use diesel::{
    deserialize::{self, FromSql},
    pg::{Pg, PgValue},
    sql_query,
    sql_types::{Text, Uuid as SqlUuid},
    OptionalExtension, QueryableByName,
};
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde::{
    de::value::{Error as ValueError, StrDeserializer},
    Deserialize, Serialize,
};
use uuid::Uuid;

use crate::db::{connection, Error, PgPool};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShipmentStatus {
    Pending,
    Packed,
    Dispatched,
    Delivered,
}

impl FromSql<Text, Pg> for ShipmentStatus {
    fn from_sql(value: PgValue<'_>) -> deserialize::Result<Self> {
        let status = std::str::from_utf8(value.as_bytes())?;
        Ok(Self::deserialize(StrDeserializer::<ValueError>::new(
            status,
        ))?)
    }
}

impl ShipmentStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Packed => "packed",
            Self::Dispatched => "dispatched",
            Self::Delivered => "delivered",
        }
    }

    pub(crate) fn can_advance_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Packed)
                | (Self::Packed, Self::Dispatched)
                | (Self::Dispatched, Self::Delivered)
        )
    }

    fn notification_type(self) -> Option<&'static str> {
        match self {
            Self::Dispatched => Some("fulfillment.shipment.dispatched.v1"),
            Self::Delivered => Some("fulfillment.shipment.delivered.v1"),
            Self::Pending | Self::Packed => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, QueryableByName)]
pub(crate) struct Shipment {
    #[diesel(sql_type = SqlUuid)]
    pub(crate) id: Uuid,
    #[diesel(sql_type = SqlUuid)]
    pub(crate) order_id: Uuid,
    #[diesel(sql_type = Text)]
    pub(crate) status: ShipmentStatus,
}

#[derive(Debug, Serialize, QueryableByName)]
pub(crate) struct ShipmentNotification {
    #[diesel(sql_type = Text)]
    event_id: String,
    #[diesel(sql_type = Text)]
    order_id: String,
    #[diesel(sql_type = Text)]
    shipment_id: String,
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = Text)]
    event_type: String,
    #[diesel(sql_type = Text)]
    occurred_at: String,
}

#[derive(QueryableByName)]
struct OrderId {
    #[diesel(sql_type = SqlUuid)]
    order_id: Uuid,
}

pub(crate) async fn consume_order(
    pool: &PgPool,
    event_id: Uuid,
    order_id: Uuid,
) -> Result<Shipment, Error> {
    connection(pool)
        .await?
        .transaction(async |tx| {
            sql_query(
                "INSERT INTO shipment (order_id) VALUES ($1) ON CONFLICT (order_id) DO NOTHING",
            )
            .bind::<SqlUuid, _>(order_id)
            .execute(tx)
            .await?;

            let inserted = sql_query(
                "INSERT INTO fulfillment_inbox (event_id, order_id) VALUES ($1, $2)
         ON CONFLICT (event_id) DO NOTHING",
            )
            .bind::<SqlUuid, _>(event_id)
            .bind::<SqlUuid, _>(order_id)
            .execute(tx)
            .await?;
            if inserted == 0 {
                let previous_order =
                    sql_query("SELECT order_id FROM fulfillment_inbox WHERE event_id = $1")
                        .bind::<SqlUuid, _>(event_id)
                        .get_result::<OrderId>(tx)
                        .await?
                        .order_id;
                if previous_order != order_id {
                    return Err(Error::InvalidArgument(
                        "order event ID already belongs to a different order".into(),
                    ));
                }
            }

            let shipment = sql_query(
                "SELECT id, order_id, status::text AS status FROM shipment WHERE order_id = $1",
            )
            .bind::<SqlUuid, _>(order_id)
            .get_result::<Shipment>(tx)
            .await?;
            Ok(shipment)
        })
        .await
}

pub(crate) async fn read_shipment(
    pool: &PgPool,
    order_id: Uuid,
) -> Result<Option<Shipment>, Error> {
    let mut conn = connection(pool).await?;
    Ok(
        sql_query("SELECT id, order_id, status::text AS status FROM shipment WHERE order_id = $1")
            .bind::<SqlUuid, _>(order_id)
            .get_result(&mut conn)
            .await
            .optional()?,
    )
}

pub(crate) async fn queue_dispatch(
    pool: &PgPool,
    order_id: Uuid,
) -> Result<Option<Shipment>, Error> {
    let mut conn = connection(pool).await?;
    Ok(sql_query(
        "UPDATE shipment
         SET dispatch_next_attempt_at = CASE WHEN status = 'packed'
             THEN COALESCE(dispatch_next_attempt_at, clock_timestamp()) END
         WHERE order_id = $1 AND status IN ('packed', 'dispatched')
         RETURNING id, order_id, status::text AS status",
    )
    .bind::<SqlUuid, _>(order_id)
    .get_result(&mut conn)
    .await
    .optional()?)
}

pub(crate) async fn next_dispatch(pool: &PgPool) -> Result<Option<Uuid>, Error> {
    // Keep the intent durable while deferring retries long enough for another order to run.
    let mut conn = connection(pool).await?;
    Ok(sql_query(
        "WITH due AS (
             SELECT order_id FROM shipment WHERE dispatch_next_attempt_at <= statement_timestamp()
             ORDER BY dispatch_next_attempt_at, order_id LIMIT 1 FOR UPDATE SKIP LOCKED
         )
         UPDATE shipment SET dispatch_next_attempt_at = clock_timestamp() + interval '5 seconds'
         FROM due WHERE shipment.order_id = due.order_id RETURNING shipment.order_id",
    )
    .get_result::<OrderId>(&mut conn)
    .await
    .optional()?
    .map(|row| row.order_id))
}

pub(crate) async fn read_notifications(
    pool: &PgPool,
    order_id: Uuid,
) -> Result<Vec<ShipmentNotification>, Error> {
    let mut conn = connection(pool).await?;
    Ok(sql_query(
        "SELECT event.event_id::text AS event_id, shipment.order_id::text AS order_id,
                shipment.id::text AS shipment_id, event.payload->>'status' AS status,
                event.event_type,
                to_char(event.occurred_at AT TIME ZONE 'UTC',
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS occurred_at
         FROM shipment JOIN fulfillment_notification_outbox event ON event.shipment_id = shipment.id
         WHERE shipment.order_id = $1 ORDER BY event.occurred_at, event.event_id",
    )
    .bind::<SqlUuid, _>(order_id)
    .load(&mut conn)
    .await?)
}

pub(crate) async fn advance_shipment(
    pool: &PgPool,
    order_id: Uuid,
    expected_status: ShipmentStatus,
    next_status: ShipmentStatus,
) -> Result<Option<Shipment>, Error> {
    if !expected_status.can_advance_to(next_status) {
        return Err(Error::InvalidArgument("invalid shipment transition".into()));
    }

    connection(pool)
        .await?
        .transaction(async |tx| {
            let shipment = sql_query(
                "UPDATE shipment SET status = $3::shipment_status, dispatch_next_attempt_at = NULL
         WHERE order_id = $1 AND status = $2::shipment_status
         RETURNING id, order_id, status::text AS status",
            )
            .bind::<SqlUuid, _>(order_id)
            .bind::<Text, _>(expected_status.as_str())
            .bind::<Text, _>(next_status.as_str())
            .get_result::<Shipment>(tx)
            .await
            .optional()?;
            if let (Some(shipment), Some(event_type)) = (&shipment, next_status.notification_type())
            {
                sql_query(
                    "INSERT INTO fulfillment_notification_outbox (shipment_id, event_type, payload)
             VALUES ($1, $2, jsonb_build_object('shipment_id', $1::uuid,
                        'order_id', $3::uuid, 'status', $4::shipment_status))",
                )
                .bind::<SqlUuid, _>(shipment.id)
                .bind::<Text, _>(event_type)
                .bind::<SqlUuid, _>(shipment.order_id)
                .bind::<Text, _>(next_status.as_str())
                .execute(tx)
                .await?;
            }
            Ok(shipment)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::{ShipmentStatus::*, *};
    use crate::db::{with_test_database, DbMigrations};

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn order_replays_preserve_shipment_and_reject_event_id_reuse() {
        with_test_database(DbMigrations::Fulfillment, async |pool| {
            let order = Uuid::from_u128(1);
            let event = Uuid::from_u128(2);
            let another_event = Uuid::from_u128(3);
            let another_order = Uuid::from_u128(4);
            assert!(read_shipment(&pool, order).await.unwrap().is_none());

            let (first, second) = tokio::join!(
                consume_order(&pool, event, order),
                consume_order(&pool, event, order),
            );
            let shipment = first.unwrap();
            assert_eq!(shipment.status, Pending);
            assert_eq!(shipment, second.unwrap());
            assert_eq!(
                shipment,
                consume_order(&pool, another_event, order).await.unwrap()
            );
            assert_eq!(Some(shipment), read_shipment(&pool, order).await.unwrap());

            assert!(matches!(
                consume_order(&pool, event, another_order).await,
                Err(Error::InvalidArgument(_))
            ));
            assert!(read_shipment(&pool, another_order).await.unwrap().is_none());
            #[derive(QueryableByName)]
            struct InboxCount {
                #[diesel(sql_type = diesel::sql_types::BigInt)]
                count: i64,
            }
            let consumed = sql_query("SELECT count(*) AS count FROM fulfillment_inbox")
                .get_result::<InboxCount>(&mut connection(&pool).await.unwrap())
                .await
                .unwrap();
            assert_eq!(consumed.count, 2);
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn shipment_transitions_compare_current_state_and_notify_once() {
        with_test_database(DbMigrations::Fulfillment, async |pool| {
            let order = Uuid::from_u128(1);
            let event = Uuid::from_u128(2);
            let shipment = consume_order(&pool, event, order).await.unwrap();
            assert!(matches!(
                advance_shipment(&pool, order, Pending, Dispatched).await,
                Err(Error::InvalidArgument(_))
            ));
            assert!(advance_shipment(&pool, order, Packed, Dispatched)
                .await
                .unwrap()
                .is_none());
            assert_eq!(
                advance_shipment(&pool, order, Pending, Packed)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                Packed
            );

            let (first, second) = tokio::join!(
                advance_shipment(&pool, order, Packed, Dispatched),
                advance_shipment(&pool, order, Packed, Dispatched),
            );
            assert_ne!(first.unwrap().is_some(), second.unwrap().is_some());
            let delivered = advance_shipment(&pool, order, Dispatched, Delivered)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (delivered.id, delivered.order_id, delivered.status),
                (shipment.id, order, Delivered)
            );
            assert_eq!(consume_order(&pool, event, order).await.unwrap(), delivered);

            let notifications = read_notifications(&pool, order).await.unwrap();
            assert_eq!(notifications.len(), 2);
            for (notification, (status, event_type)) in notifications.iter().zip([
                ("dispatched", "fulfillment.shipment.dispatched.v1"),
                ("delivered", "fulfillment.shipment.delivered.v1"),
            ]) {
                assert_eq!(notification.shipment_id, shipment.id.to_string());
                assert_eq!(notification.order_id, order.to_string());
                assert_eq!(notification.status, status);
                assert_eq!(notification.event_type, event_type);
                assert!(Uuid::parse_str(&notification.event_id).is_ok());
                assert!(notification.occurred_at.ends_with('Z'));
            }
            assert_ne!(notifications[0].event_id, notifications[1].event_id);
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn dispatch_intents_survive_worker_retries_and_clear_with_shipment() {
        with_test_database(DbMigrations::Fulfillment, async |pool| {
            let order = Uuid::from_u128(1);
            let another_order = Uuid::from_u128(3);
            consume_order(&pool, Uuid::from_u128(2), order)
                .await
                .unwrap();
            consume_order(&pool, Uuid::from_u128(4), another_order)
                .await
                .unwrap();
            assert!(queue_dispatch(&pool, order).await.unwrap().is_none());
            assert!(next_dispatch(&pool).await.unwrap().is_none());
            let packed = advance_shipment(&pool, order, Pending, Packed)
                .await
                .unwrap()
                .unwrap();
            advance_shipment(&pool, another_order, Pending, Packed)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(queue_dispatch(&pool, order).await.unwrap(), Some(packed));
            queue_dispatch(&pool, order).await.unwrap();
            queue_dispatch(&pool, another_order).await.unwrap();

            let (first, second) = tokio::join!(next_dispatch(&pool), next_dispatch(&pool));
            let mut claimed = [first.unwrap().unwrap(), second.unwrap().unwrap()];
            claimed.sort_unstable();
            assert_eq!(claimed, [order, another_order]);
            assert!(next_dispatch(&pool).await.unwrap().is_none());

            // Advance the retry time without sleeping for the worker lease window.
            sql_query(
                "UPDATE shipment SET dispatch_next_attempt_at = clock_timestamp()
                 WHERE order_id = $1",
            )
            .bind::<SqlUuid, _>(order)
            .execute(&mut connection(&pool).await.unwrap())
            .await
            .unwrap();
            assert_eq!(next_dispatch(&pool).await.unwrap(), Some(order));
            let dispatched = advance_shipment(&pool, order, Packed, Dispatched)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                queue_dispatch(&pool, order).await.unwrap(),
                Some(dispatched)
            );
            assert_eq!(read_notifications(&pool, order).await.unwrap().len(), 1);
            #[derive(QueryableByName)]
            struct DispatchQueue {
                #[diesel(sql_type = diesel::sql_types::Bool)]
                queued: bool,
            }
            let dispatch = sql_query(
                "SELECT dispatch_next_attempt_at IS NOT NULL AS queued
                 FROM shipment WHERE order_id = $1",
            )
            .bind::<SqlUuid, _>(order)
            .get_result::<DispatchQueue>(&mut connection(&pool).await.unwrap())
            .await
            .unwrap();
            assert!(!dispatch.queued);
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires a PostgreSQL test server"]
    async fn notification_failure_rolls_back_shipment_transition() {
        with_test_database(DbMigrations::Fulfillment, async |pool| {
            let order = Uuid::from_u128(1);
            consume_order(&pool, Uuid::from_u128(2), order)
                .await
                .unwrap();
            let packed = advance_shipment(&pool, order, Pending, Packed)
                .await
                .unwrap()
                .unwrap();
            queue_dispatch(&pool, order).await.unwrap();
            sql_query(
                "INSERT INTO fulfillment_notification_outbox (shipment_id, event_type, payload)
             VALUES ($1, 'fulfillment.shipment.dispatched.v1', '{}'::jsonb)",
            )
            .bind::<SqlUuid, _>(packed.id)
            .execute(&mut connection(&pool).await.unwrap())
            .await
            .unwrap();

            let error = advance_shipment(&pool, order, Packed, Dispatched)
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                Error::Query(diesel::result::Error::DatabaseError(
                    diesel::result::DatabaseErrorKind::UniqueViolation,
                    _
                ))
            ));
            assert_eq!(read_shipment(&pool, order).await.unwrap(), Some(packed));
            assert_eq!(next_dispatch(&pool).await.unwrap(), Some(order));
        })
        .await;
    }
}
