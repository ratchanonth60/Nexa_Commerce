// DEMO-ONLY public catalog read. No authentication, rate limiting or production telemetry.
use axum::{
    extract::{Path, Query, State},
    http::{HeaderValue, Method, StatusCode},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Uuid, FromRow, PgPool};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info};

#[derive(Clone)]
struct AppState {
    db: PgPool,
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

fn catalog_app(db: PgPool) -> Router {
    // Allow only our local development web UI. Do not use wildcard origins in production.
    let allowed_origins = [
        HeaderValue::from_static("http://localhost:5173"),
        HeaderValue::from_static("http://127.0.0.1:5173"),
    ];
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/catalog/products", get(list_products))
        .route("/api/v1/catalog/products/{productId}", get(get_product))
        .layer(
            CorsLayer::new()
                .allow_origin(allowed_origins)
                .allow_methods([Method::GET]),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(AppState { db })
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

    let app = catalog_app(db);
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

    #[tokio::test]
    async fn rejects_invalid_product_ids() {
        let db = PgPool::connect_lazy("postgres://localhost/catalog_db").unwrap();
        let response = catalog_app(db)
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
        let db = PgPool::connect_lazy("postgres://localhost/catalog_db").unwrap();
        let app = catalog_app(db);

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
