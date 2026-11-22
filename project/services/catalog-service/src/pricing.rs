use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use diesel::{
    sql_query,
    sql_types::{Integer, Text, Uuid as SqlUuid},
    OptionalExtension, QueryableByName,
};
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};
use tracing::error;
use uuid::Uuid;

use crate::{db, AppState};

#[derive(Deserialize)]
pub(crate) struct QuoteQuery {
    product_id: String,
    currency: String,
    channel: String,
}

#[derive(Serialize, QueryableByName)]
pub(crate) struct PriceQuote {
    #[diesel(sql_type = Text)]
    quote_id: String,
    #[diesel(sql_type = Text)]
    product_id: String,
    #[diesel(sql_type = Text)]
    currency: String,
    #[diesel(sql_type = Text)]
    unit_amount_minor: String,
    #[diesel(sql_type = Text)]
    expires_at: String,
    #[diesel(sql_type = Integer)]
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

    let mut conn = db::connection(&state.pricing_db)
        .await
        .map_err(query_error)?;
    sql_query(
        "SELECT gen_random_uuid()::text AS quote_id, product_id::text AS product_id,
            currency, unit_amount_minor::text AS unit_amount_minor, rule_version,
            to_char((now() + interval '5 minutes') AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS expires_at
         FROM price WHERE product_id = $1 AND currency = $2 AND channel = $3",
    )
    .bind::<SqlUuid, _>(product_id)
    .bind::<Text, _>(query.currency)
    .bind::<Text, _>(query.channel)
    .get_result::<PriceQuote>(&mut conn)
    .await
    .optional()
    .map_err(|err| query_error(err.into()))?
    .map(Json)
    .ok_or(StatusCode::NOT_FOUND)
}

fn query_error(err: db::Error) -> StatusCode {
    error!(error = %err, "pricing query failed");
    StatusCode::INTERNAL_SERVER_ERROR
}
