use axum::{http::header, response::Html, routing::get, Router};

pub(crate) fn routes() -> Router {
    Router::new()
        .route("/docs", get(|| async { Html(include_str!("docs.html")) }))
        .route(
            "/openapi.yaml",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
                    include_str!("../../../contracts/nexa_gateway.openapi.yaml"),
                )
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_documentation_and_the_embedded_contract() {
        for (path, content_type, expected) in [
            ("/docs", "text/html", "SwaggerUIBundle"),
            (
                "/openapi.yaml",
                "application/yaml",
                include_str!("../../../contracts/nexa_gateway.openapi.yaml"),
            ),
        ] {
            let response = routes()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with(content_type));
            let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
            assert!(std::str::from_utf8(&body).unwrap().contains(expected));
        }
    }
}
