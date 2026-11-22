use serde::{Deserialize, Serialize};
use sqlx::{types::Uuid, Error, PgPool};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "shipment_status", rename_all = "snake_case")]
pub(crate) enum ShipmentStatus {
    Pending,
    Packed,
    Dispatched,
    Delivered,
}

impl ShipmentStatus {
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

#[derive(Debug, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub(crate) struct Shipment {
    pub(crate) id: Uuid,
    pub(crate) order_id: Uuid,
    pub(crate) status: ShipmentStatus,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub(crate) struct ShipmentNotification {
    event_id: String,
    order_id: String,
    shipment_id: String,
    status: String,
    event_type: String,
    occurred_at: String,
}

pub(crate) async fn consume_order(
    pool: &PgPool,
    event_id: Uuid,
    order_id: Uuid,
) -> Result<Shipment, Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shipment (order_id) VALUES ($1) ON CONFLICT (order_id) DO NOTHING")
        .bind(order_id)
        .execute(&mut *tx)
        .await?;

    let inserted = sqlx::query(
        "INSERT INTO fulfillment_inbox (event_id, order_id) VALUES ($1, $2)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event_id)
    .bind(order_id)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let previous_order: Uuid =
            sqlx::query_scalar("SELECT order_id FROM fulfillment_inbox WHERE event_id = $1")
                .bind(event_id)
                .fetch_one(&mut *tx)
                .await?;
        if previous_order != order_id {
            tx.rollback().await?;
            return Err(Error::InvalidArgument(
                "order event ID already belongs to a different order".into(),
            ));
        }
    }

    let shipment = sqlx::query_as("SELECT id, order_id, status FROM shipment WHERE order_id = $1")
        .bind(order_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(shipment)
}

pub(crate) async fn read_shipment(
    pool: &PgPool,
    order_id: Uuid,
) -> Result<Option<Shipment>, Error> {
    sqlx::query_as("SELECT id, order_id, status FROM shipment WHERE order_id = $1")
        .bind(order_id)
        .fetch_optional(pool)
        .await
}

pub(crate) async fn queue_dispatch(
    pool: &PgPool,
    order_id: Uuid,
) -> Result<Option<Shipment>, Error> {
    sqlx::query_as(
        "UPDATE shipment
         SET dispatch_next_attempt_at = CASE WHEN status = 'packed'
             THEN COALESCE(dispatch_next_attempt_at, clock_timestamp()) END
         WHERE order_id = $1 AND status IN ('packed', 'dispatched')
         RETURNING id, order_id, status",
    )
    .bind(order_id)
    .fetch_optional(pool)
    .await
}

pub(crate) async fn next_dispatch(pool: &PgPool) -> Result<Option<Uuid>, Error> {
    // Keep the intent durable while deferring retries long enough for another order to run.
    sqlx::query_scalar(
        "WITH due AS (
             SELECT order_id FROM shipment WHERE dispatch_next_attempt_at <= statement_timestamp()
             ORDER BY dispatch_next_attempt_at, order_id LIMIT 1 FOR UPDATE SKIP LOCKED
         )
         UPDATE shipment SET dispatch_next_attempt_at = clock_timestamp() + interval '5 seconds'
         FROM due WHERE shipment.order_id = due.order_id RETURNING shipment.order_id",
    )
    .fetch_optional(pool)
    .await
}

pub(crate) async fn read_notifications(
    pool: &PgPool,
    order_id: Uuid,
) -> Result<Vec<ShipmentNotification>, Error> {
    sqlx::query_as(
        "SELECT event.event_id::text AS event_id, shipment.order_id::text AS order_id,
                shipment.id::text AS shipment_id, event.payload->>'status' AS status,
                event.event_type,
                to_char(event.occurred_at AT TIME ZONE 'UTC',
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS occurred_at
         FROM shipment JOIN fulfillment_notification_outbox event ON event.shipment_id = shipment.id
         WHERE shipment.order_id = $1 ORDER BY event.occurred_at, event.event_id",
    )
    .bind(order_id)
    .fetch_all(pool)
    .await
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

    let mut tx = pool.begin().await?;
    let shipment = sqlx::query_as::<_, Shipment>(
        "UPDATE shipment SET status = $3, dispatch_next_attempt_at = NULL
         WHERE order_id = $1 AND status = $2
         RETURNING id, order_id, status",
    )
    .bind(order_id)
    .bind(expected_status)
    .bind(next_status)
    .fetch_optional(&mut *tx)
    .await?;
    if let (Some(shipment), Some(event_type)) = (&shipment, next_status.notification_type()) {
        sqlx::query(
            "INSERT INTO fulfillment_notification_outbox (shipment_id, event_type, payload)
             VALUES ($1, $2, jsonb_build_object('shipment_id', $1::uuid,
                        'order_id', $3::uuid, 'status', $4::shipment_status))",
        )
        .bind(shipment.id)
        .bind(event_type)
        .bind(shipment.order_id)
        .bind(next_status)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(shipment)
}

#[cfg(test)]
mod tests {
    use super::{ShipmentStatus::*, *};

    #[sqlx::test(migrations = "./fulfillment-migrations")]
    #[ignore = "requires a PostgreSQL test server"]
    async fn order_replays_preserve_shipment_and_reject_event_id_reuse(pool: PgPool) {
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
        let consumed = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fulfillment_inbox")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(consumed, 2);
    }

    #[sqlx::test(migrations = "./fulfillment-migrations")]
    #[ignore = "requires a PostgreSQL test server"]
    async fn shipment_transitions_compare_current_state_and_notify_once(pool: PgPool) {
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
    }

    #[sqlx::test(migrations = "./fulfillment-migrations")]
    #[ignore = "requires a PostgreSQL test server"]
    async fn dispatch_intents_survive_worker_retries_and_clear_with_shipment(pool: PgPool) {
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
        sqlx::query(
            "UPDATE shipment SET dispatch_next_attempt_at = clock_timestamp() WHERE order_id = $1",
        )
        .bind(order)
        .execute(&pool)
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
        let queued: bool = sqlx::query_scalar(
            "SELECT dispatch_next_attempt_at IS NOT NULL FROM shipment WHERE order_id = $1",
        )
        .bind(order)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!queued);
    }

    #[sqlx::test(migrations = "./fulfillment-migrations")]
    #[ignore = "requires a PostgreSQL test server"]
    async fn notification_failure_rolls_back_shipment_transition(pool: PgPool) {
        let order = Uuid::from_u128(1);
        consume_order(&pool, Uuid::from_u128(2), order)
            .await
            .unwrap();
        let packed = advance_shipment(&pool, order, Pending, Packed)
            .await
            .unwrap()
            .unwrap();
        queue_dispatch(&pool, order).await.unwrap();
        sqlx::query(
            "INSERT INTO fulfillment_notification_outbox (shipment_id, event_type, payload)
             VALUES ($1, 'fulfillment.shipment.dispatched.v1', '{}'::jsonb)",
        )
        .bind(packed.id)
        .execute(&pool)
        .await
        .unwrap();

        let error = advance_shipment(&pool, order, Packed, Dispatched)
            .await
            .unwrap_err();
        assert!(error.as_database_error().unwrap().is_unique_violation());
        assert_eq!(read_shipment(&pool, order).await.unwrap(), Some(packed));
        assert_eq!(next_dispatch(&pool).await.unwrap(), Some(order));
    }
}
