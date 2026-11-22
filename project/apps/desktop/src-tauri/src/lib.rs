use serde::{Deserialize, Serialize};

mod auth;
mod cart;
mod operations;

use auth::{get_session, sign_in, sign_out, DesktopSession};
use cart::{add_cart_item, create_cart, get_cart};
use operations::{adjust_stock, advance_shipment, read_shipment, read_stock};

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
async fn search_products(query: String) -> Result<Vec<ProductDto>, String> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| "Could not initialize the search client.")?
        .get("http://127.0.0.1:8082/api/v1/search/products")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|_| "Could not reach product search.")?
        .error_for_status()
        .map_err(|_| "Product search failed.")?
        .json()
        .await
        .map_err(|_| "Product search returned an invalid response.".into())
}

#[tauri::command]
async fn get_product(product_id: String) -> Result<Option<ProductDto>, String> {
    let product_id = uuid::Uuid::parse_str(&product_id).map_err(|err| err.to_string())?;
    let response = reqwest::Client::new()
        .get(format!("{PRODUCTS_URL}/{product_id}"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    optional_json(response).await
}

#[derive(Serialize, Deserialize)]
struct PriceQuote {
    quote_id: String,
    product_id: String,
    currency: String,
    unit_amount_minor: String,
    expires_at: String,
    rule_version: i32,
}

#[tauri::command]
async fn get_price_quote(
    product_id: String,
    currency: String,
    channel: String,
) -> Result<Option<PriceQuote>, String> {
    let product_id = uuid::Uuid::parse_str(&product_id)
        .map_err(|err| err.to_string())?
        .to_string();
    let response = reqwest::Client::new()
        .get("http://127.0.0.1:8082/api/v1/pricing/quotes")
        .query(&[
            ("product_id", product_id),
            ("currency", currency),
            ("channel", channel),
        ])
        .send()
        .await
        .map_err(|err| err.to_string())?;
    optional_json(response).await
}

#[derive(Serialize, Deserialize)]
struct Availability {
    product_id: String,
    quantity: i32,
    available: bool,
}

#[tauri::command]
async fn get_availability(
    product_id: String,
    quantity: i32,
) -> Result<Option<Availability>, String> {
    let product_id = uuid::Uuid::parse_str(&product_id)
        .map_err(|err| err.to_string())?
        .to_string();
    let response = reqwest::Client::new()
        .get("http://127.0.0.1:8082/api/v1/inventory/availability")
        .query(&[
            ("product_id", product_id),
            ("quantity", quantity.to_string()),
        ])
        .send()
        .await
        .map_err(|err| err.to_string())?;
    optional_json(response).await
}

async fn optional_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<Option<T>, String> {
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response
        .error_for_status()
        .map_err(|err| err.to_string())?
        .json::<T>()
        .await
        .map(Some)
        .map_err(|err| err.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DesktopSession::default())
        .invoke_handler(tauri::generate_handler![
            list_products,
            search_products,
            get_product,
            get_price_quote,
            get_availability,
            get_session,
            sign_in,
            sign_out,
            create_cart,
            get_cart,
            add_cart_item,
            read_stock,
            adjust_stock,
            read_shipment,
            advance_shipment
        ])
        .run(tauri::generate_context!())
        .expect("error while running Nexa Commerce desktop");
}
