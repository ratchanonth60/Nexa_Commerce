use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;
use tracing::error;

use crate::AppState;

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
        "SELECT on_hand - reserved >= $2 FROM stock_record WHERE product_id = $1",
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
