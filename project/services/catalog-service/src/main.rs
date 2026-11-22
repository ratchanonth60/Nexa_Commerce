// Development gateway for Catalog, Pricing, Inventory and Cart.
mod auth;
mod cart;
mod inventory;
mod pricing;
mod web_session;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderValue, Method, StatusCode},
    middleware,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Uuid, FromRow, PgPool};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};

#[derive(Clone)]
struct AppState {
    db: PgPool,
    pricing_db: PgPool,
    inventory_db: PgPool,
    cart_db: PgPool,
    access_tokens: Option<auth::AccessTokens>,
    web_sessions: Option<std::sync::Arc<web_session::WebSessions>>,
}

#[derive(Serialize, FromRow)]
struct ProductDto {
    id: String,
    sku: String,
    title: String,
    status: String,
}

#[derive(Deserialize)]
struct ProductQuery {
    limit: Option<i64>,
}

async fn health() -> &'static str {
    "ok"
}

async fn list_products(
    State(state): State<AppState>,
    Query(query): Query<ProductQuery>,
) -> Result<Json<Vec<ProductDto>>, StatusCode> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let products = sqlx::query_as::<_, ProductDto>(
        "SELECT id::text AS id, sku, title, status FROM product WHERE status = 'active' ORDER BY title, id LIMIT $1"
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(catalog_query_error)?;
    Ok(Json(products))
}

async fn get_product(
    State(state): State<AppState>,
    Path(product_id): Path<String>,
) -> Result<Json<ProductDto>, StatusCode> {
    let product_id = Uuid::parse_str(&product_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    sqlx::query_as::<_, ProductDto>(
        "SELECT id::text AS id, sku, title, status FROM product WHERE id = $1 AND status = 'active'",
    )
    .bind(product_id)
    .fetch_optional(&state.db)
    .await
    .map_err(catalog_query_error)?
    .map(Json)
    .ok_or(StatusCode::NOT_FOUND)
}

fn catalog_query_error(err: sqlx::Error) -> StatusCode {
    error!(error = %err, "catalog query failed");
    StatusCode::INTERNAL_SERVER_ERROR
}

fn commerce_app(state: AppState) -> Router {
    // Allow only our local development web UI. Do not use wildcard origins in production.
    let allowed_origins = [
        HeaderValue::from_static("http://localhost:5173"),
        HeaderValue::from_static("http://127.0.0.1:5173"),
    ];
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/catalog/products", get(list_products))
        .route("/api/v1/catalog/products/{productId}", get(get_product))
        .route("/api/v1/pricing/quotes", get(pricing::get_quote))
        .route(
            "/api/v1/inventory/availability",
            get(inventory::get_availability),
        )
        .merge(
            Router::new()
                .route("/api/v1/carts", post(cart::create_cart))
                .route("/api/v1/carts/{cartId}", get(cart::read_cart))
                .route("/api/v1/carts/{cartId}/items", post(cart::add_item))
                .route("/auth/login", get(web_session::login))
                .route("/auth/callback", get(web_session::callback))
                .route("/auth/session", get(web_session::session))
                .route("/auth/logout", post(web_session::logout))
                .route_layer(middleware::from_fn(web_session::private_response)),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(allowed_origins)
                .allow_methods([Method::GET]),
        )
        .layer(TraceLayer::new_for_http().make_span_with(
            |request: &axum::extract::Request| {
                tracing::debug_span!("request", method = %request.method(), path = %request.uri().path())
            },
        ))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")?;
    let db = PgPool::connect(&database_url).await?;
    sqlx::migrate!().run(&db).await?;

    let pricing_db = PgPool::connect(&std::env::var("PRICING_DATABASE_URL")?).await?;
    sqlx::migrate!("./pricing-migrations")
        .run(&pricing_db)
        .await?;

    let inventory_db = PgPool::connect(&std::env::var("INVENTORY_DATABASE_URL")?).await?;
    sqlx::migrate!("./inventory-migrations")
        .run(&inventory_db)
        .await?;

    let cart_db = PgPool::connect(&std::env::var("CART_DATABASE_URL")?).await?;
    sqlx::migrate!("./cart-migrations").run(&cart_db).await?;
    let access_tokens = auth::AccessTokens::from_env()?;
    let web_sessions = web_session::WebSessions::from_env(access_tokens.as_ref())?;

    let app = commerce_app(AppState {
        db,
        pricing_db,
        inventory_db,
        cart_db,
        access_tokens,
        web_sessions,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8082").await?;
    info!("catalog service listening on 127.0.0.1:8082");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{header, Request},
    };
    use tower::ServiceExt;

    fn test_app() -> Router {
        let db = PgPool::connect_lazy("postgres://localhost/catalog_db").unwrap();
        commerce_app(AppState {
            db: db.clone(),
            pricing_db: db.clone(),
            inventory_db: db.clone(),
            cart_db: db,
            access_tokens: None,
            web_sessions: None,
        })
    }

    #[tokio::test]
    async fn rejects_invalid_product_ids() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/catalog/products/not-a-uuid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_invalid_limits_and_allows_the_local_storefront() {
        let app = test_app();

        for (query, origin) in [
            ("0", "http://localhost:5173"),
            ("101", "http://127.0.0.1:5173"),
            ("bogus", "https://example.com"),
        ] {
            let request = Request::builder()
                .uri(format!("/api/v1/catalog/products?limit={query}"))
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let allowed = response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN);
            if origin == "https://example.com" {
                assert!(allowed.is_none());
            } else {
                assert_eq!(allowed.unwrap(), origin);
            }
        }
    }
}
