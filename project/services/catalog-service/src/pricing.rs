use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Uuid, FromRow};
use tracing::error;

use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct QuoteQuery {
    product_id: String,
    currency: String,
    channel: String,
}

#[derive(Serialize, FromRow)]
pub(crate) struct PriceQuote {
    quote_id: String,
    product_id: String,
    currency: String,
    unit_amount_minor: String,
    expires_at: String,
    rule_version: i32,
}

pub(crate) async fn get_quote(
    State(state): State<AppState>,
    Query(query): Query<QuoteQuery>,
) -> Result<Json<PriceQuote>, StatusCode> {
    let product_id = Uuid::parse_str(&query.product_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    if query.currency.len() != 3
        || !query.currency.bytes().all(|byte| byte.is_ascii_uppercase())
        || query.channel.is_empty()
        || query.channel.len() > 32
        || !query
            .channel
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    sqlx::query_as::<_, PriceQuote>(
        "SELECT gen_random_uuid()::text AS quote_id, product_id::text AS product_id,
            currency, unit_amount_minor::text AS unit_amount_minor, rule_version,
            to_char((now() + interval '5 minutes') AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS expires_at
         FROM price WHERE product_id = $1 AND currency = $2 AND channel = $3",
    )
    .bind(product_id)
    .bind(query.currency)
    .bind(query.channel)
    .fetch_optional(&state.pricing_db)
    .await
    .map_err(|err| {
        error!(error = %err, "pricing query failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map(Json)
    .ok_or(StatusCode::NOT_FOUND)
}
