use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use diesel::{
    sql_query,
    sql_types::{BigInt, Text},
};
use diesel_async::RunQueryDsl;
use serde::Deserialize;

use crate::{
    catalog_query_error,
    db::{self, PgPool},
    AppState, ProductDto,
};

const MAX_QUERY_LENGTH: usize = 120;

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

pub(crate) async fn search_products(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<ProductDto>>, StatusCode> {
    let terms = query.q.trim();
    let limit = query.limit.unwrap_or(20);
    if terms.is_empty()
        || query.q.chars().take(MAX_QUERY_LENGTH + 1).count() > MAX_QUERY_LENGTH
        || terms.contains('\0')
        || !(1..=100).contains(&limit)
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    find_products(&state.db, terms, limit)
        .await
        .map(Json)
        .map_err(catalog_query_error)
}

async fn find_products(
    pool: &PgPool,
    terms: &str,
    limit: i64,
) -> Result<Vec<ProductDto>, db::Error> {
    // The document expression and active filter match the Catalog GIN index.
    let mut conn = db::connection(pool).await?;
    Ok(sql_query(
        "WITH search AS (SELECT plainto_tsquery('simple', $1) AS terms)
         SELECT id::text AS id, sku, title, status
         FROM product CROSS JOIN search
         WHERE status = 'active'
           AND to_tsvector('simple', title || ' ' || sku) @@ search.terms
         ORDER BY ts_rank(to_tsvector('simple', title || ' ' || sku), search.terms) DESC,
                  title, product.id
         LIMIT $2",
    )
    .bind::<Text, _>(terms)
    .bind::<BigInt, _>(limit)
    .load(&mut conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a PostgreSQL test server"]
    async fn searches_title_and_sku_and_excludes_inactive_products() {
        db::with_test_database(db::DbMigrations::Catalog, async |pool| {
        let mug = find_products(&pool, "nEXa Mug", 20).await.unwrap();
        assert_eq!(mug.len(), 1);
        assert_eq!(mug[0].sku, "DEMO-MUG");

        let shirt = find_products(&pool, "dEmO-TsHiRt", 20).await.unwrap();
        assert_eq!(shirt.len(), 1);
        assert_eq!(shirt[0].sku, "DEMO-TSHIRT");

        let mut conn = db::connection(&pool).await.unwrap();
        sql_query(
            "INSERT INTO product (id, sku, title, status) VALUES
             ('019b0000-0000-7000-8000-000000000003', 'HIDDEN-DRAFT', 'Nexa Demo draft', 'draft'),
             ('019b0000-0000-7000-8000-000000000004', 'HIDDEN-ARCHIVE', 'Nexa Demo archive', 'archived')",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        let demo = find_products(&pool, "demo", 20).await.unwrap();
        assert_eq!(
            demo.iter()
                .map(|product| product.sku.as_str())
                .collect::<Vec<_>>(),
            ["DEMO-MUG", "DEMO-TSHIRT"]
        );
        let limited = find_products(&pool, "demo", 1).await.unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, demo[0].id);
        assert!(find_products(&pool, "no matching title", 20)
            .await
            .unwrap()
            .is_empty());
        }).await;
    }
}
