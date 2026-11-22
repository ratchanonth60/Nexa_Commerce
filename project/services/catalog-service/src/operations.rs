use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use diesel::{
    sql_query,
    sql_types::{Bool, Text, Uuid as SqlUuid},
    OptionalExtension, QueryableByName,
};
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;

use crate::{
    auth::Customer,
    db::{connection, Error, PgPool},
    fulfillment::{self, Shipment, ShipmentNotification, ShipmentStatus},
    inventory, AppState,
};

pub(crate) async fn read_customer_shipment(
    customer: Customer,
    State(state): State<AppState>,
    Path(order_id): Path<Uuid>,
) -> Result<Json<Shipment>, StatusCode> {
    require_order_owner(&state, customer, order_id).await?;
    read_shipment(&state, order_id).await
}

pub(crate) async fn read_notifications(
    customer: Customer,
    State(state): State<AppState>,
    Path(order_id): Path<Uuid>,
) -> Result<Json<Vec<ShipmentNotification>>, StatusCode> {
    require_order_owner(&state, customer, order_id).await?;
    let pool = state
        .fulfillment
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    fulfillment::read_notifications(pool, order_id)
        .await
        .map(Json)
        .map_err(operation_error)
}

async fn require_order_owner(
    state: &AppState,
    customer: Customer,
    order_id: Uuid,
) -> Result<(), StatusCode> {
    let order = state
        .checkout
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    #[derive(QueryableByName)]
    struct Ownership {
        #[diesel(sql_type = Bool)]
        owns_order: bool,
    }
    let mut conn = connection(order.order_pool())
        .await
        .map_err(operation_error)?;
    let ownership = sql_query(
        "SELECT EXISTS (
            SELECT 1 FROM checkout c JOIN order_customer owner ON owner.id = c.customer_id
            WHERE c.id = $1 AND c.status = 'confirmed' AND owner.issuer = $2 AND owner.subject = $3
         ) AS owns_order",
    )
    .bind::<SqlUuid, _>(order_id)
    .bind::<Text, _>(customer.issuer)
    .bind::<Text, _>(customer.subject)
    .get_result::<Ownership>(&mut conn)
    .await
    .map_err(|err| operation_error(err.into()))?;
    if !ownership.owns_order {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

pub(crate) async fn read_operator_shipment(
    customer: Customer,
    State(state): State<AppState>,
    Path(order_id): Path<Uuid>,
) -> Result<Json<Shipment>, StatusCode> {
    if !customer.operator {
        return Err(StatusCode::FORBIDDEN);
    }
    read_shipment(&state, order_id).await
}

async fn read_shipment(state: &AppState, order_id: Uuid) -> Result<Json<Shipment>, StatusCode> {
    let pool = state
        .fulfillment
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    fulfillment::read_shipment(pool, order_id)
        .await
        .map_err(operation_error)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
pub(crate) struct ShipmentChange {
    expected_status: ShipmentStatus,
    status: ShipmentStatus,
}

pub(crate) async fn advance_shipment(
    customer: Customer,
    State(state): State<AppState>,
    Path(order_id): Path<Uuid>,
    Json(change): Json<ShipmentChange>,
) -> Result<(StatusCode, Json<Shipment>), StatusCode> {
    if !customer.operator {
        return Err(StatusCode::FORBIDDEN);
    }
    let pool = state
        .fulfillment
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let shipment = read_shipment(&state, order_id).await?;
    if !change.expected_status.can_advance_to(change.status) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if shipment.status == change.status {
        return Ok((StatusCode::OK, shipment));
    }
    if shipment.status != change.expected_status {
        return Err(StatusCode::CONFLICT);
    }
    if change.status == ShipmentStatus::Dispatched {
        let queued = fulfillment::queue_dispatch(pool, order_id)
            .await
            .map_err(operation_error)?
            .ok_or(StatusCode::CONFLICT)?;
        let status = if queued.status == ShipmentStatus::Dispatched {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        return Ok((status, Json(queued)));
    }

    match fulfillment::advance_shipment(pool, order_id, change.expected_status, change.status)
        .await
        .map_err(operation_error)?
    {
        Some(shipment) => Ok((StatusCode::OK, Json(shipment))),
        None => {
            let current = read_shipment(&state, order_id).await?;
            if current.status == change.status {
                Ok((StatusCode::OK, current))
            } else {
                Err(StatusCode::CONFLICT)
            }
        }
    }
}

pub(crate) async fn deliver_orders(order_db: PgPool, fulfillment_db: PgPool, inventory_db: PgPool) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        interval.tick().await;
        if let Err(err) = deliver_order(&order_db, &fulfillment_db).await {
            error!(error = %err, "order event delivery failed");
        }
        if let Err(err) = dispatch_order(&fulfillment_db, &inventory_db).await {
            error!(error = %err, "queued shipment dispatch failed");
        }
    }
}

async fn dispatch_order(fulfillment_db: &PgPool, inventory_db: &PgPool) -> Result<(), Error> {
    let Some(order_id) = fulfillment::next_dispatch(fulfillment_db).await? else {
        return Ok(());
    };
    if !inventory::dispatch(inventory_db, order_id).await? {
        return Err(Error::InvalidArgument(
            "shipment has no allocated inventory".into(),
        ));
    }
    fulfillment::advance_shipment(
        fulfillment_db,
        order_id,
        ShipmentStatus::Packed,
        ShipmentStatus::Dispatched,
    )
    .await?;
    Ok(())
}

async fn deliver_order(order_db: &PgPool, fulfillment_db: &PgPool) -> Result<(), Error> {
    #[derive(QueryableByName)]
    struct OrderEvent {
        #[diesel(sql_type = SqlUuid)]
        event_id: Uuid,
        #[diesel(sql_type = SqlUuid)]
        order_id: Uuid,
    }
    connection(order_db)
        .await?
        .transaction(async |tx| {
            let event = sql_query(
                "SELECT event_id, order_id FROM order_outbox WHERE fulfilled_at IS NULL
             ORDER BY occurred_at, event_id LIMIT 1 FOR UPDATE SKIP LOCKED",
            )
            .get_result::<OrderEvent>(tx)
            .await
            .optional()?;
            if let Some(event) = event {
                fulfillment::consume_order(fulfillment_db, event.event_id, event.order_id).await?;
                sql_query("UPDATE order_outbox SET fulfilled_at = now() WHERE event_id = $1")
                    .bind::<SqlUuid, _>(event.event_id)
                    .execute(tx)
                    .await?;
            }
            Ok(())
        })
        .await
}

fn operation_error(err: Error) -> StatusCode {
    error!(error = %err, "shipment operation failed");
    StatusCode::INTERNAL_SERVER_ERROR
}
