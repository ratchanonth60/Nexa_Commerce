use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Uuid, PgExecutor};
use tracing::error;

use crate::{auth::Customer, catalog_query_error, AppState};

#[derive(Serialize)]
pub(crate) struct Cart {
    id: String,
    version: i64,
    items: Vec<CartItem>,
}

#[derive(Serialize)]
struct CartItem {
    product_id: String,
    quantity: i32,
}

#[derive(Deserialize)]
pub(crate) struct AddCartItem {
    product_id: String,
    quantity: i32,
}

pub(crate) async fn create_cart(
    customer: Customer,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<Cart>), StatusCode> {
    let (id, version) = sqlx::query_as::<_, (Uuid, i64)>(
        "INSERT INTO cart (owner_issuer, owner_subject) VALUES ($1, $2) RETURNING id, version",
    )
    .bind(&customer.issuer)
    .bind(&customer.subject)
    .fetch_one(&state.cart_db)
    .await
    .map_err(cart_query_error)?;

    Ok((
        StatusCode::CREATED,
        Json(Cart {
            id: id.to_string(),
            version,
            items: Vec::new(),
        }),
    ))
}

pub(crate) async fn read_cart(
    customer: Customer,
    State(state): State<AppState>,
    Path(cart_id): Path<String>,
) -> Result<Json<Cart>, StatusCode> {
    let cart_id = Uuid::parse_str(&cart_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    load_cart(&state.cart_db, cart_id, &customer)
        .await
        .map(Json)
}

pub(crate) async fn add_item(
    customer: Customer,
    State(state): State<AppState>,
    Path(cart_id): Path<String>,
    body: Result<Json<AddCartItem>, JsonRejection>,
) -> Result<Json<Cart>, StatusCode> {
    let cart_id = Uuid::parse_str(&cart_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let Json(item) = body.map_err(|_| StatusCode::BAD_REQUEST)?;
    let product_id = Uuid::parse_str(&item.product_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    if !(1..=9999).contains(&item.quantity) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut tx = state.cart_db.begin().await.map_err(cart_query_error)?;
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM cart WHERE id = $1 AND owner_issuer = $2 AND owner_subject = $3 FOR UPDATE",
    )
    .bind(cart_id)
    .bind(&customer.issuer)
    .bind(&customer.subject)
    .fetch_optional(&mut *tx)
    .await
    .map_err(cart_query_error)?
    .ok_or(StatusCode::FORBIDDEN)?;

    let product_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM product WHERE id = $1 AND status = 'active')",
    )
    .bind(product_id)
    .fetch_one(&state.db)
    .await
    .map_err(catalog_query_error)?;
    if !product_exists {
        return Err(StatusCode::NOT_FOUND);
    }

    sqlx::query_scalar::<_, i32>(
        "INSERT INTO cart_item (cart_id, product_id, quantity) VALUES ($1, $2, $3)
         ON CONFLICT (cart_id, product_id) DO UPDATE
         SET quantity = cart_item.quantity + EXCLUDED.quantity
         WHERE cart_item.quantity + EXCLUDED.quantity <= 9999
         RETURNING quantity",
    )
    .bind(cart_id)
    .bind(product_id)
    .bind(item.quantity)
    .fetch_optional(&mut *tx)
    .await
    .map_err(cart_query_error)?
    .ok_or(StatusCode::CONFLICT)?;

    sqlx::query("UPDATE cart SET version = version + 1 WHERE id = $1")
        .bind(cart_id)
        .execute(&mut *tx)
        .await
        .map_err(cart_query_error)?;

    let cart = load_cart(&mut *tx, cart_id, &customer).await?;
    tx.commit().await.map_err(cart_query_error)?;
    Ok(Json(cart))
}

async fn load_cart<'e>(
    db: impl PgExecutor<'e>,
    cart_id: Uuid,
    customer: &Customer,
) -> Result<Cart, StatusCode> {
    // Read the version and items in one statement so they share a database snapshot.
    let rows = sqlx::query_as::<_, (i64, Option<Uuid>, Option<i32>)>(
        "SELECT cart.version, cart_item.product_id, cart_item.quantity
         FROM cart
         LEFT JOIN cart_item ON cart_item.cart_id = cart.id
         WHERE cart.id = $1 AND cart.owner_issuer = $2 AND cart.owner_subject = $3
         ORDER BY cart_item.product_id",
    )
    .bind(cart_id)
    .bind(&customer.issuer)
    .bind(&customer.subject)
    .fetch_all(db)
    .await
    .map_err(cart_query_error)?;

    let version = rows.first().ok_or(StatusCode::FORBIDDEN)?.0;
    let items = rows
        .into_iter()
        .filter_map(|(_, product_id, quantity)| {
            product_id
                .zip(quantity)
                .map(|(product_id, quantity)| CartItem {
                    product_id: product_id.to_string(),
                    quantity,
                })
        })
        .collect();

    Ok(Cart {
        id: cart_id.to_string(),
        version,
        items,
    })
}

fn cart_query_error(err: sqlx::Error) -> StatusCode {
    error!(error = %err, "cart query failed");
    StatusCode::INTERNAL_SERVER_ERROR
}
