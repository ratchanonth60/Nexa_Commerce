use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::StatusCode,
    Json,
};
use diesel::{
    sql_query,
    sql_types::{BigInt, Bool, Integer, Nullable, Text, Uuid as SqlUuid},
    QueryableByName,
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde::{Deserialize, Serialize};
use tracing::error;
use uuid::Uuid;

use crate::{
    auth::Customer,
    catalog_query_error,
    db::{connection, Error},
    AppState,
};

#[derive(QueryableByName)]
struct CartHeader {
    #[diesel(sql_type = SqlUuid)]
    id: Uuid,
    #[diesel(sql_type = BigInt)]
    version: i64,
}

#[derive(QueryableByName)]
struct Exists {
    #[diesel(sql_type = Bool)]
    found: bool,
}

#[derive(QueryableByName)]
struct SnapshotRow {
    #[diesel(sql_type = BigInt)]
    version: i64,
    #[diesel(sql_type = Nullable<SqlUuid>)]
    product_id: Option<Uuid>,
    #[diesel(sql_type = Nullable<Integer>)]
    quantity: Option<i32>,
}

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
    let mut db = connection(&state.cart_db).await.map_err(cart_query_error)?;
    let CartHeader { id, version } = sql_query(
        "INSERT INTO cart (owner_issuer, owner_subject) VALUES ($1, $2) RETURNING id, version",
    )
    .bind::<Text, _>(&customer.issuer)
    .bind::<Text, _>(&customer.subject)
    .get_result(&mut db)
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
    let mut db = connection(&state.cart_db).await.map_err(cart_query_error)?;
    load_cart(&mut db, cart_id, &customer).await.map(Json)
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

    let mut db = connection(&state.cart_db).await.map_err(cart_query_error)?;
    db.transaction::<_, Error, _>(async |tx| {
        let owned = sql_query(
            "SELECT EXISTS (
                SELECT 1 FROM cart WHERE id = $1 AND owner_issuer = $2 AND owner_subject = $3 FOR UPDATE
             ) AS found",
        )
        .bind::<SqlUuid, _>(cart_id)
        .bind::<Text, _>(&customer.issuer)
        .bind::<Text, _>(&customer.subject)
        .get_result::<Exists>(tx)
        .await?;
        if !owned.found {
            return Err(StatusCode::FORBIDDEN.into());
        }

        let product = sql_query(
            "SELECT EXISTS (SELECT 1 FROM product WHERE id = $1 AND status = 'active') AS found",
        )
        .bind::<SqlUuid, _>(product_id)
        .get_result::<Exists>(&mut *connection(&state.db).await.map_err(catalog_query_error)?)
        .await
        .map_err(|err| catalog_query_error(Error::from(err)))?;
        if !product.found {
            return Err(StatusCode::NOT_FOUND.into());
        }

        let updated = sql_query(
            "INSERT INTO cart_item (cart_id, product_id, quantity) VALUES ($1, $2, $3)
             ON CONFLICT (cart_id, product_id) DO UPDATE
             SET quantity = cart_item.quantity + EXCLUDED.quantity
             WHERE cart_item.quantity + EXCLUDED.quantity <= 9999",
        )
        .bind::<SqlUuid, _>(cart_id)
        .bind::<SqlUuid, _>(product_id)
        .bind::<Integer, _>(item.quantity)
        .execute(tx)
        .await?;
        if updated == 0 {
            return Err(StatusCode::CONFLICT.into());
        }

        sql_query("UPDATE cart SET version = version + 1 WHERE id = $1")
            .bind::<SqlUuid, _>(cart_id)
            .execute(tx)
            .await?;

        Ok(Json(load_cart(tx, cart_id, &customer).await?))
    })
    .await
    .map_err(|err| err.into_status(cart_query_error))
}

async fn load_cart(
    db: &mut AsyncPgConnection,
    cart_id: Uuid,
    customer: &Customer,
) -> Result<Cart, StatusCode> {
    let (version, items) = checkout_snapshot(db, cart_id, customer).await?;
    Ok(Cart {
        id: cart_id.to_string(),
        version,
        items: items
            .into_iter()
            .map(|(product_id, quantity)| CartItem {
                product_id: product_id.to_string(),
                quantity,
            })
            .collect(),
    })
}

pub(crate) async fn checkout_snapshot(
    db: &mut AsyncPgConnection,
    cart_id: Uuid,
    customer: &Customer,
) -> Result<(i64, Vec<(Uuid, i32)>), StatusCode> {
    // Read the version and items in one statement so they share a database snapshot.
    let rows = sql_query(
        "SELECT cart.version, cart_item.product_id, cart_item.quantity
         FROM cart
         LEFT JOIN cart_item ON cart_item.cart_id = cart.id
         WHERE cart.id = $1 AND cart.owner_issuer = $2 AND cart.owner_subject = $3
         ORDER BY cart_item.product_id",
    )
    .bind::<SqlUuid, _>(cart_id)
    .bind::<Text, _>(&customer.issuer)
    .bind::<Text, _>(&customer.subject)
    .load::<SnapshotRow>(db)
    .await
    .map_err(cart_query_error)?;

    let version = rows.first().ok_or(StatusCode::FORBIDDEN)?.version;
    let items = rows
        .into_iter()
        .filter_map(|row| row.product_id.zip(row.quantity))
        .collect();
    Ok((version, items))
}

fn cart_query_error(err: impl std::fmt::Display) -> StatusCode {
    error!(error = %err, "cart query failed");
    StatusCode::INTERNAL_SERVER_ERROR
}
