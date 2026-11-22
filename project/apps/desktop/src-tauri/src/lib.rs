use serde::{Deserialize, Serialize};

const PRODUCTS_URL: &str = "http://127.0.0.1:8082/api/v1/catalog/products";

#[derive(Clone, Serialize, Deserialize)]
struct ProductDto {
    id: String,
    sku: String,
    title: String,
    status: String,
}

#[tauri::command]
async fn list_products() -> Result<Vec<ProductDto>, String> {
    // Development-only. Production: use OIDC/PKCE tokens in Rust secure storage,
    // configure the HTTPS Gateway endpoint, timeouts, correlation IDs, and error mapping.
    let response = reqwest::Client::new()
        .get(PRODUCTS_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    response
        .json::<Vec<ProductDto>>()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_product(product_id: String) -> Result<Option<ProductDto>, String> {
    let product_id = uuid::Uuid::parse_str(&product_id).map_err(|err| err.to_string())?;
    let response = reqwest::Client::new()
        .get(format!("{PRODUCTS_URL}/{product_id}"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response
        .error_for_status()
        .map_err(|err| err.to_string())?
        .json::<ProductDto>()
        .await
        .map(Some)
        .map_err(|err| err.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![list_products, get_product])
        .run(tauri::generate_context!())
        .expect("error while running Nexa Commerce desktop");
}
